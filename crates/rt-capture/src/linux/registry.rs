//! The PipeWire connection. A thread of its own runs a PipeWire main loop with the registry, every playback stream's
//! volume, our capture streams and their links. PipeWire objects can't leave that thread, so everything else sends it
//! [`Command`]s through a [`Client`] and waits for the answer.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use pipewire as pw;
use pw::core::{CoreRc, PW_ID_CORE};
use pw::link::Link;
use pw::main_loop::MainLoopRc;
use pw::node::{Node, NodeChangeMask, NodeListener};
use pw::properties::PropertiesBox;
use pw::registry::{GlobalObject, RegistryRc};
use pw::spa::param::ParamType;
use pw::spa::utils::dict::DictRef;
use pw::types::ObjectType;

use super::apps::{self, AppGroup, AppKey, Own, Placed, Placement, ProcFs, StreamInfo};
use super::capture::{self, CaptureStream, PLAYBACK, PortInfo};
use super::volume::{self, NodeSession, NodeVolume};
use super::{guard, icon};
use crate::before_duck::{BeforeDuck, Session};
use crate::duck_journal::{self, JournalEntry};
use crate::ramp;
use crate::{AudioApp, CaptureError, DuckLevel, FrameSink, Result};

/// How long a caller waits for the loop, and the loop for the daemon while it sets up.
pub(super) const TIMEOUT: Duration = Duration::from_secs(5);

/// How long a stream must have been around before a volume that doesn't match the duck journal counts as the user's.
/// WirePlumber gives a new stream its remembered volume only once it sees it, a moment after we do.
const SETTLE: Duration = Duration::from_secs(2);

pub(super) type Reply<T> = mpsc::Sender<Result<T>>;

/// What the loop is asked to do.
pub(super) enum Command {
    ListApps(Reply<Vec<AudioApp>>),
    /// Starts capturing `app` into `sink`, compensated for `level`; `running` turns false when the capture ends by
    /// itself. Answers with the capture's id.
    Start {
        app: AudioApp,
        sink: FrameSink,
        level: DuckLevel,
        running: Arc<AtomicBool>,
        reply: Reply<u64>,
    },
    Stop {
        id: u64,
        reply: Reply<()>,
    },
    /// One step of a fade of `app` (see `ramp::run`). `first` marks the first step, where a volume the user changed
    /// during the duck is found and left alone. `duck_level` learns when the step reaches the audio.
    Fade {
        app: AudioApp,
        level: f32,
        duck_new: bool,
        first: bool,
        duck_level: DuckLevel,
        reply: Reply<()>,
    },
    /// Gives every ducked stream its volume back at once.
    RestoreAll(Reply<()>),
    Quit,
}

/// The loop thread, ended when this is dropped, once the daemon has read everything it was sent.
pub(super) struct Connection {
    client: Client,
    thread: Option<JoinHandle<()>>,
}

/// Sends commands to the loop thread, from any thread.
#[derive(Clone)]
pub(super) struct Client {
    commands: pw::channel::Sender<Command>,
    alive: Arc<AtomicBool>,
}

impl Connection {
    /// Connects to the daemon at `remote` on a new thread. With a duck `journal`, ducks an earlier run left behind
    /// come back first, for the streams there are. Each capture stores the length of the graph's cycles in `quantum`.
    pub fn open(remote: PathBuf, journal: Option<PathBuf>, quantum: Arc<AtomicU64>) -> Result<Self> {
        let (commands, receiver) = pw::channel::channel();
        let (ready, setup) = mpsc::channel();
        let alive = Arc::new(AtomicBool::new(true));
        let flag = Arc::clone(&alive);
        let thread = thread::Builder::new()
            .name("rt-pipewire".into())
            .spawn(move || run(remote, journal, quantum, receiver, ready, flag))
            .map_err(|e| CaptureError::Backend(format!("can't start the PipeWire thread: {e}")))?;
        let client = Client { commands, alive };
        match setup.recv_timeout(TIMEOUT) {
            Ok(Ok(())) => Ok(Self {
                client,
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                // Left to end by itself; joining could hang as long as the daemon does.
                let _ = client.commands.send(Command::Quit);
                Err(CaptureError::Backend("PipeWire didn't answer".into()))
            }
        }
    }

    pub fn is_alive(&self) -> bool {
        self.client.is_alive() && self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }

    pub fn client(&self) -> Client {
        self.client.clone()
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.client.commands.send(Command::Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Client {
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    /// Sends the command `make` builds around a reply channel, and waits for the answer.
    pub fn request<T>(&self, make: impl FnOnce(Reply<T>) -> Command) -> Result<T> {
        let lost = || CaptureError::Backend("the connection to PipeWire was lost".into());
        if !self.is_alive() {
            return Err(lost());
        }
        let (reply, answer) = mpsc::channel();
        self.commands.send(make(reply)).map_err(|_| lost())?;
        match answer.recv_timeout(TIMEOUT) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => Err(CaptureError::Backend("PipeWire didn't answer".into())),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(lost()),
        }
    }
}

/// Clears the alive flag when the loop thread ends, however it ends.
struct Alive(Arc<AtomicBool>);

impl Drop for Alive {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

/// The loop thread.
fn run(
    remote: PathBuf,
    journal: Option<PathBuf>,
    quantum: Arc<AtomicU64>,
    commands: pw::channel::Receiver<Command>,
    ready: mpsc::Sender<Result<()>>,
    alive: Arc<AtomicBool>,
) {
    let _alive = Alive(alive);
    pw::init();
    let backend = |what: &str, e: pw::Error| CaptureError::Backend(format!("{what}: {e}"));
    let setup = || -> Result<(MainLoopRc, CoreRc, RegistryRc)> {
        let mainloop = MainLoopRc::new(None).map_err(|e| backend("PipeWire main loop", e))?;
        let context = pw::context::ContextRc::new(&mainloop, None).map_err(|e| backend("PipeWire context", e))?;
        let mut props = PropertiesBox::new();
        props.insert("remote.name", remote.to_string_lossy().as_ref());
        // In a Flatpak, WirePlumber gives a client in this category the right to link streams and set their volume.
        props.insert("media.category", "Manager");
        let core = context
            .connect_rc(Some(props))
            .map_err(|e| backend(&format!("can't connect to PipeWire at {}", remote.display()), e))?;
        let registry = core.get_registry_rc().map_err(|e| backend("PipeWire registry", e))?;
        Ok((mainloop, core, registry))
    };
    let (mainloop, core, registry) = match setup() {
        Ok(objects) => objects,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };

    let state = Rc::new(RefCell::new(State::new(
        core.clone(),
        registry.clone(),
        journal,
        quantum,
    )));
    let weak = Rc::downgrade(&state);
    let _registry_listener = registry
        .add_listener_local()
        .global({
            let weak = weak.clone();
            move |global| {
                if let Some(state) = weak.upgrade() {
                    state.borrow_mut().added(global, &weak);
                }
            }
        })
        .global_remove({
            let weak = weak.clone();
            move |id| {
                if let Some(state) = weak.upgrade() {
                    state.borrow_mut().removed(id);
                }
            }
        })
        .register();

    // A roundtrip waits for `done` with the sequence number of its `sync`. `EPIPE` on the core means the connection
    // broke: the daemon went away.
    let waiting: Rc<Cell<Option<i32>>> = Rc::default();
    let broken: Rc<Cell<bool>> = Rc::default();
    let _core_listener = core
        .add_listener_local()
        .done({
            let (waiting, mainloop) = (Rc::clone(&waiting), mainloop.downgrade());
            move |id, seq| {
                if id == PW_ID_CORE && waiting.get() == Some(seq.seq()) {
                    waiting.set(None);
                    if let Some(m) = mainloop.upgrade() {
                        m.quit();
                    }
                }
            }
        })
        .error({
            let (broken, mainloop) = (Rc::clone(&broken), mainloop.downgrade());
            move |id, _, res, _| {
                // Other errors on the core answer a request of ours, e.g. to destroy a link that went meanwhile.
                if id == PW_ID_CORE && res == -libc::EPIPE {
                    broken.set(true);
                    if let Some(m) = mainloop.upgrade() {
                        m.quit();
                    }
                }
            }
        })
        .register();
    let roundtrip = || -> bool {
        let Ok(seq) = core.sync(0) else { return false };
        waiting.set(Some(seq.seq()));
        let timed_out = Rc::new(Cell::new(false));
        let timer = mainloop.loop_().add_timer({
            let (timed_out, mainloop) = (Rc::clone(&timed_out), mainloop.downgrade());
            move |_| {
                timed_out.set(true);
                if let Some(m) = mainloop.upgrade() {
                    m.quit();
                }
            }
        });
        let _ = timer.update_timer(Some(TIMEOUT), None);
        mainloop.run();
        !broken.get() && !timed_out.get() && waiting.get().is_none()
    };
    // The first roundtrip brings every object there is; the second the volumes of the streams bound meanwhile.
    if !(roundtrip() && roundtrip()) {
        let _ = ready.send(Err(CaptureError::Backend(
            "PipeWire didn't finish setting up the connection".into(),
        )));
        return;
    }
    state.borrow_mut().recover();
    let _ = ready.send(Ok(()));

    let _commands = commands.attach(mainloop.loop_(), {
        let (weak, mainloop) = (weak.clone(), mainloop.downgrade());
        move |command| match command {
            Command::Quit => {
                if let Some(m) = mainloop.upgrade() {
                    m.quit();
                }
            }
            command => {
                if let Some(state) = weak.upgrade() {
                    state.borrow_mut().handle(command);
                }
            }
        }
    });
    if !broken.get() {
        mainloop.run();
    }
    state.borrow_mut().end_captures();
    // The daemon ends a client that hung up before it reads what is still in the client's socket, so what we sent last,
    // like the volumes a restore set, is lost if we hang up before the daemon got to it. The duck guard hangs up right
    // after it restores. A last roundtrip waits until the daemon has read everything.
    if !broken.get() {
        roundtrip();
    }
}

/// What a client says about the app behind it.
#[derive(Debug, Default)]
struct ClientInfo {
    sec_pid: Option<u32>,
    app_pid: Option<u32>,
    pulse: bool,
    binary: Option<String>,
    name: Option<String>,
}

/// A node, and for a playback stream what we track of it.
struct NodeEntry {
    class: String,
    stream: Option<Stream>,
}

/// A playback stream, bound so we can read and set its volume.
struct Stream {
    /// `object.serial`, which unlike the id is never reused: what the duck state knows the stream by.
    serial: String,
    /// What the node itself says, and its client: the two together are its [`StreamInfo`].
    own: StreamInfo,
    client: Option<u32>,
    /// The node and its client together; filled in, and placed, once the client is known.
    info: StreamInfo,
    placement: Option<Placement>,
    /// What the duck journal knows it by, see [`volume::identifier`].
    identifier: Option<String>,
    /// `node.async`: the stream makes each cycle's audio in the cycle before.
    asynchronous: bool,
    /// When we first saw it.
    seen: Instant,
    volume: RefCell<NodeVolume>,
    // Dropped before the proxy it listens to.
    _listener: NodeListener,
    proxy: Node,
}

/// A running capture.
struct Capture {
    id: u64,
    key: AppKey,
    /// The app's process, whose end ends the capture once its streams are gone. 0 if unknown.
    pid: u32,
    node_name: String,
    /// Our capture node, once the registry announced it.
    node: Option<u32>,
    links: BTreeMap<(u32, u32), Link>,
    running: Arc<AtomicBool>,
    /// Whether the app's streams are asynchronous, for the capture's compensation.
    asynchronous: Arc<AtomicBool>,
    stream: Option<CaptureStream>,
}

/// Ducked streams, and the journal that lets a later run restore them after a crash.
#[derive(Default)]
struct Ducking {
    before_duck: BeforeDuck,
    journal: Option<PathBuf>,
    /// Entries from an earlier run's journal that no stream has matched yet.
    pending: Vec<JournalEntry>,
    /// What the journal holds, to skip writes that change nothing.
    written: Option<String>,
    /// The level new streams of a ducked app join at.
    join: HashMap<AppKey, f32>,
}

impl Ducking {
    /// Writes the ducked and pending streams to the journal. Failures are ignored: the journal only helps after a
    /// crash, and the next change tries again.
    fn save(&mut self) {
        let Some(path) = &self.journal else { return };
        let text = duck_journal::serialize(self.before_duck.entries().chain(&self.pending));
        if self.written.as_ref() != Some(&text) && duck_journal::write(path, &text).is_ok() {
            self.written = Some(text);
        }
    }
}

struct State {
    core: CoreRc,
    registry: RegistryRc,
    own: Own,
    clients: HashMap<u32, ClientInfo>,
    nodes: HashMap<u32, NodeEntry>,
    ports: HashMap<u32, PortInfo>,
    captures: Vec<Capture>,
    next_capture: u64,
    ducking: Ducking,
    quantum: Arc<AtomicU64>,
}

fn number(props: &DictRef, key: &str) -> Option<u32> {
    props.get(key)?.trim().parse().ok()
}

fn text(props: &DictRef, key: &str) -> Option<String> {
    props.get(key).filter(|v| !v.is_empty()).map(str::to_owned)
}

impl State {
    fn new(core: CoreRc, registry: RegistryRc, journal: Option<PathBuf>, quantum: Arc<AtomicU64>) -> Self {
        let pending = journal.as_deref().map(duck_journal::read).unwrap_or_default();
        Self {
            core,
            registry,
            own: Own::new(&ProcFs),
            clients: HashMap::new(),
            nodes: HashMap::new(),
            ports: HashMap::new(),
            captures: Vec::new(),
            next_capture: 0,
            ducking: Ducking {
                journal,
                pending,
                ..Default::default()
            },
            quantum,
        }
    }

    fn added(&mut self, global: &GlobalObject<&DictRef>, weak: &Weak<RefCell<State>>) {
        let Some(props) = global.props else { return };
        match global.type_ {
            ObjectType::Client => {
                let info = ClientInfo {
                    sec_pid: number(props, "pipewire.sec.pid"),
                    app_pid: number(props, "application.process.id"),
                    pulse: props.get("client.api") == Some("pipewire-pulse"),
                    binary: text(props, "application.process.binary"),
                    name: text(props, "application.name"),
                };
                self.clients.insert(global.id, info);
                // Streams announced before their client.
                let clients = &self.clients;
                for stream in self.nodes.values_mut().filter_map(|n| n.stream.as_mut()) {
                    if stream.client == Some(global.id) {
                        stream.place(clients);
                    }
                }
                self.relink();
            }
            ObjectType::Node => {
                let class = props.get("media.class").unwrap_or_default().to_owned();
                let name = text(props, "node.name");
                let stream = if class == PLAYBACK {
                    self.watch(global, props, weak)
                } else {
                    None
                };
                for capture in &mut self.captures {
                    if name.as_deref() == Some(capture.node_name.as_str()) {
                        capture.node = Some(global.id);
                    }
                }
                self.nodes.insert(global.id, NodeEntry { class, stream });
                self.relink();
            }
            ObjectType::Port => {
                let (Some(node), Some(direction)) = (number(props, "node.id"), props.get("port.direction")) else {
                    return;
                };
                let port = PortInfo {
                    node,
                    output: direction == "out",
                    channel: props.get("audio.channel").unwrap_or_default().to_owned(),
                    audio: props.get("format.dsp").is_none_or(|f| f.contains("audio")),
                    monitor: props.get("port.monitor") == Some("true"),
                };
                self.ports.insert(global.id, port);
                self.relink();
            }
            _ => {}
        }
    }

    /// Binds a playback stream and subscribes to its volume.
    fn watch(&self, global: &GlobalObject<&DictRef>, props: &DictRef, weak: &Weak<RefCell<State>>) -> Option<Stream> {
        let proxy: Node = self.registry.bind(global).ok()?;
        let id = global.id;
        // The registry gives a short list of properties; the node's info has them all, the app's PID among them.
        let listener = proxy
            .add_listener_local()
            .info({
                let weak = weak.clone();
                move |info| {
                    if info.change_mask().contains(NodeChangeMask::PROPS)
                        && let Some(props) = info.props()
                        && let Some(state) = weak.upgrade()
                    {
                        state.borrow_mut().node_info(id, props);
                    }
                }
            })
            .param({
                let weak = weak.clone();
                move |_, kind, _, _, param| {
                    if kind != ParamType::Props {
                        return;
                    }
                    if let Some(channels) = param.and_then(volume::channel_volumes)
                        && let Some(state) = weak.upgrade()
                    {
                        state.borrow_mut().volume_reported(id, channels);
                    }
                }
            })
            .register();
        proxy.subscribe_params(&[ParamType::Props]);
        let mut stream = Stream {
            identifier: volume::identifier(|key| props.get(key)),
            asynchronous: props.get("node.async") == Some("true"),
            serial: props
                .get("object.serial")
                .map_or_else(|| format!("id {id}"), str::to_owned),
            own: StreamInfo {
                client_pid: None,
                app_pid: number(props, "application.process.id"),
                pulse: props.get("client.api") == Some("pipewire-pulse"),
                binary: text(props, "application.process.binary"),
                name: text(props, "application.name"),
            },
            client: number(props, "client.id"),
            info: StreamInfo::default(),
            placement: None,
            seen: Instant::now(),
            volume: RefCell::default(),
            _listener: listener,
            proxy,
        };
        stream.place(&self.clients);
        Some(stream)
    }

    /// A stream's full properties arrived (or changed).
    fn node_info(&mut self, id: u32, props: &DictRef) {
        let Some(stream) = self.nodes.get_mut(&id).and_then(|n| n.stream.as_mut()) else {
            return;
        };
        let own = &mut stream.own;
        own.app_pid = number(props, "application.process.id").or(own.app_pid);
        own.pulse |= props.get("client.api") == Some("pipewire-pulse");
        own.binary = text(props, "application.process.binary").or(own.binary.take());
        own.name = text(props, "application.name").or(own.name.take());
        stream.client = number(props, "client.id").or(stream.client);
        stream.identifier = volume::identifier(|key| props.get(key)).or(stream.identifier.take());
        stream.asynchronous = props.get("node.async") == Some("true");
        stream.place(&self.clients);
        self.relink();
    }

    fn removed(&mut self, id: u32) {
        self.clients.remove(&id);
        self.ports.remove(&id);
        if self.nodes.remove(&id).is_some() {
            // Captures whose own node went (it failed), or whose app is gone: no streams left, and no process either.
            let nodes = &self.nodes;
            let ended: Vec<u64> = self
                .captures
                .iter()
                .filter(|c| {
                    let streams_left = nodes.values().any(|n| key_of(n) == Some(&c.key));
                    let process_left = c.pid != 0 && guard::alive(c.pid as libc::pid_t);
                    c.node == Some(id) || (!streams_left && !process_left)
                })
                .map(|c| c.id)
                .collect();
            for id in ended {
                self.stop(id);
            }
        }
        self.relink();
    }

    /// Links every capture to the output ports of its app's streams, and drops links whose ports are gone.
    fn relink(&mut self) {
        let State {
            core,
            nodes,
            ports,
            captures,
            ..
        } = self;
        for capture in captures.iter_mut() {
            let Some(node) = capture.node else { continue };
            let app: BTreeSet<u32> = nodes
                .iter()
                .filter(|(_, n)| key_of(n) == Some(&capture.key))
                .map(|(&id, _)| id)
                .collect();
            let asynchronous = app.iter().any(|id| {
                nodes
                    .get(id)
                    .and_then(|n| n.stream.as_ref())
                    .is_some_and(|s| s.asynchronous)
            });
            capture.asynchronous.store(asynchronous, Ordering::Relaxed);
            let wanted = capture::plan_links(ports, &app, |id| nodes.get(&id).map(|n| n.class.as_str()), node);
            let stale: Vec<(u32, u32)> = capture.links.keys().filter(|l| !wanted.contains(l)).copied().collect();
            for ends in stale {
                let Some(link) = capture.links.remove(&ends) else {
                    continue;
                };
                // A link whose port went is gone already; only the proxy is left to drop.
                if ports.contains_key(&ends.0) && ports.contains_key(&ends.1) {
                    let _ = core.destroy_object(link);
                }
            }
            for &(output, input) in &wanted {
                if capture.links.contains_key(&(output, input)) {
                    continue;
                }
                let Some(output_node) = ports.get(&output).map(|p| p.node) else {
                    continue;
                };
                let mut props = PropertiesBox::new();
                props.insert("link.output.node", output_node.to_string());
                props.insert("link.output.port", output.to_string());
                props.insert("link.input.node", node.to_string());
                props.insert("link.input.port", input.to_string());
                props.insert("link.passive", "true");
                props.insert("object.linger", "false");
                if let Ok(link) = core.create_object::<Link>("link-factory", &props) {
                    capture.links.insert((output, input), link);
                }
            }
        }
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::ListApps(reply) => {
                self.recover();
                let _ = reply.send(Ok(self.groups().into_iter().map(|g| g.app).collect()));
            }
            Command::Start {
                app,
                sink,
                level,
                running,
                reply,
            } => {
                let _ = reply.send(self.start(&app, sink, level, running));
            }
            Command::Stop { id, reply } => {
                self.stop(id);
                let _ = reply.send(Ok(()));
            }
            Command::Fade {
                app,
                level,
                duck_new,
                first,
                duck_level,
                reply,
            } => {
                let _ = reply.send(self.fade_step(&app, level, duck_new, first, &duck_level));
            }
            Command::RestoreAll(reply) => {
                self.restore_all();
                let _ = reply.send(Ok(()));
            }
            Command::Quit => {}
        }
    }

    /// The apps there are now.
    fn groups(&self) -> Vec<AppGroup> {
        let placed: Vec<Placed> = self
            .nodes
            .iter()
            .filter_map(|(&node, entry)| {
                let stream = entry.stream.as_ref()?;
                Some(Placed {
                    node,
                    info: stream.info.clone(),
                    placement: stream.placement.clone()?,
                })
            })
            .collect();
        apps::group(&placed, self.own, icon::desktop_name)
    }

    fn start(&mut self, app: &AudioApp, sink: FrameSink, level: DuckLevel, running: Arc<AtomicBool>) -> Result<u64> {
        let groups = self.groups();
        let group = apps::find(&groups, app).ok_or(CaptureError::AppNotFound(app.pid))?;
        self.next_capture += 1;
        let id = self.next_capture;
        let node_name = format!("repeat-that-capture-{}-{id}", std::process::id());
        let asynchronous = Arc::new(AtomicBool::new(false));
        let stream = capture::open(
            &self.core,
            &node_name,
            sink,
            level,
            Arc::clone(&running),
            Arc::clone(&self.quantum),
            Arc::clone(&asynchronous),
        )
        .map_err(|e| CaptureError::Backend(format!("can't open a capture stream: {e}")))?;
        self.captures.push(Capture {
            id,
            key: group.key.clone(),
            pid: group.app.pid,
            node_name,
            node: None,
            links: BTreeMap::new(),
            running,
            asynchronous,
            stream: Some(stream),
        });
        self.relink();
        Ok(id)
    }

    fn stop(&mut self, id: u64) {
        let Some(i) = self.captures.iter().position(|c| c.id == id) else {
            return;
        };
        let mut capture = self.captures.remove(i);
        capture.running.store(false, Ordering::Relaxed);
        // The stream first, so nothing is captured through links that are about to go.
        capture.stream.take();
        for (ends, link) in std::mem::take(&mut capture.links) {
            if self.ports.contains_key(&ends.0) && self.ports.contains_key(&ends.1) {
                let _ = self.core.destroy_object(link);
            }
        }
    }

    fn end_captures(&mut self) {
        let ids: Vec<u64> = self.captures.iter().map(|c| c.id).collect();
        for id in ids {
            self.stop(id);
        }
    }

    fn fade_step(
        &mut self,
        app: &AudioApp,
        level: f32,
        duck_new: bool,
        first: bool,
        duck_level: &DuckLevel,
    ) -> Result<()> {
        let groups = self.groups();
        let key = apps::find(&groups, app)
            .ok_or(CaptureError::AppNotFound(app.pid))?
            .key
            .clone();
        let State { nodes, ducking, .. } = self;
        let streams: Vec<&Stream> = nodes
            .values()
            .filter(|n| key_of(n) == Some(&key))
            .filter_map(|n| n.stream.as_ref())
            .collect();
        if first {
            // A stream whose volume the user changed during the duck keeps it: it isn't ours to restore any more.
            for stream in &streams {
                if let Some(entry) = ducking.before_duck.get(&stream.serial)
                    && volume::changed_by_user(entry, &stream.volume.borrow())
                {
                    ducking.before_duck.restored(&stream.serial);
                }
            }
        }
        adopt(&mut ducking.before_duck, &streams);
        let sessions: Vec<NodeSession<'_>> = streams.iter().filter_map(|s| session(s)).collect();
        ramp::apply(&sessions, level, duck_new, &mut ducking.before_duck);
        // Captures switch their compensation for audio played from the next cycle on.
        duck_level.set(level, capture::now() + capture::VOLUME_DELAY);
        if duck_new && level < 1.0 {
            ducking.join.insert(key, level);
        } else {
            ducking.join.remove(&key);
        }
        // Every step: recovery only restores streams still at the volume the journal holds.
        ducking.save();
        Ok(())
    }

    fn restore_all(&mut self) {
        let State { nodes, ducking, .. } = self;
        for stream in nodes.values().filter_map(|n| n.stream.as_ref()) {
            if ducking.before_duck.contains(&stream.serial)
                && let Some(session) = session(stream)
            {
                ducking.before_duck.apply(&session, 1.0);
            }
        }
        ducking.join.clear();
        ducking.save();
    }

    /// A stream reported its volume.
    fn volume_reported(&mut self, id: u32, channels: Vec<f32>) {
        let Some(stream) = self.nodes.get(&id).and_then(|n| n.stream.as_ref()) else {
            return;
        };
        let first = stream.volume.borrow().level().is_none();
        stream.volume.borrow_mut().reported(channels);
        // WirePlumber gives a stream that comes back the volume it remembers, ducked if we died while ducking.
        self.recover();
        // A new stream of a ducked app is ducked at once, rather than playing at full volume (and being boosted by the
        // capture's compensation too).
        let State { nodes, ducking, .. } = self;
        let Some(stream) = nodes.get(&id).and_then(|n| n.stream.as_ref()) else {
            return;
        };
        if adopt(&mut ducking.before_duck, &[stream]) {
            ducking.save();
        }
        if first
            && let Some(&level) = stream.placement.as_ref().and_then(|p| ducking.join.get(&p.key))
            && !ducking.before_duck.contains(&stream.serial)
            && let Some(session) = session(stream)
        {
            ducking.before_duck.apply(&session, level);
            ducking.save();
        }
    }

    /// Restores the streams that ducks from an earlier run's journal still hold down.
    fn recover(&mut self) {
        if self.ducking.pending.is_empty() {
            return;
        }
        let State { nodes, ducking, .. } = self;
        // Streams this run ducked aren't the earlier run's.
        let streams: Vec<&Stream> = nodes
            .values()
            .filter_map(|n| n.stream.as_ref())
            .filter(|s| !ducking.before_duck.contains(&s.serial))
            .collect();
        let known: Vec<(&Stream, String, f32, bool)> = streams
            .iter()
            .filter_map(|s| {
                let level = s.volume.borrow().level()?;
                let settled = s.seen.elapsed() >= SETTLE && s.volume.borrow().settled();
                Some((*s, s.identifier.clone()?, level, settled))
            })
            .collect();
        let pairs: Vec<(String, f32)> = known.iter().map(|(_, id, level, _)| (id.clone(), *level)).collect();
        let (restore, _) = volume::recover(ducking.pending.clone(), &pairs);
        for &(i, original) in &restore {
            let (stream, ..) = known[i];
            if let Some(session) = session(stream) {
                session.set_volume(original);
            }
        }
        // An entry is done once a stream of its app got its volume back, or has kept another volume for a while.
        ducking.pending.retain(|entry| {
            let restored = restore.iter().any(|&(i, _)| known[i].1 == entry.identifier);
            let kept = known
                .iter()
                .any(|(_, id, _, settled)| *settled && *id == entry.identifier);
            !restored && !kept
        });
        ducking.save();
    }
}

impl Stream {
    /// Works out what the stream is and where it belongs, once its client is known (or it has none).
    fn place(&mut self, clients: &HashMap<u32, ClientInfo>) {
        let client = match self.client {
            Some(id) => match clients.get(&id) {
                Some(client) => Some(client),
                None => return,
            },
            None => None,
        };
        let own = &self.own;
        self.info = StreamInfo {
            client_pid: client.and_then(|c| c.sec_pid),
            app_pid: own.app_pid.or(client.and_then(|c| c.app_pid)),
            pulse: own.pulse || client.is_some_and(|c| c.pulse),
            binary: own.binary.clone().or_else(|| client.and_then(|c| c.binary.clone())),
            name: own.name.clone().or_else(|| client.and_then(|c| c.name.clone())),
        };
        self.placement = apps::place(&self.info, &ProcFs);
    }
}

/// Takes streams that aren't ducked by us but sit at the ducked volume of another of ours with the same identifier as
/// ducked from that one's volume, and says whether there were any. WirePlumber gives a new stream the volume it
/// remembers for its identifier, which is the ducked one while we duck a stream with that identifier; ducking it from
/// there would duck it twice, and restoring would leave it ducked.
fn adopt(before_duck: &mut BeforeDuck, streams: &[&Stream]) -> bool {
    let mut adopted = false;
    for stream in streams {
        let (Some(identifier), Some(level)) = (&stream.identifier, stream.volume.borrow().level()) else {
            continue;
        };
        if before_duck.contains(&stream.serial) {
            continue;
        }
        let sibling = before_duck
            .entries()
            .find(|e| e.identifier == *identifier && duck_journal::should_restore(e, level))
            .map(|e| e.original);
        if let Some(original) = sibling {
            let entry = JournalEntry {
                identifier: identifier.clone(),
                original,
                ducked: level,
            };
            before_duck.adopt(&stream.serial, entry);
            adopted = true;
        }
    }
    adopted
}

/// The app a node's stream belongs to, if it is a playback stream that could be placed.
fn key_of(node: &NodeEntry) -> Option<&AppKey> {
    Some(&node.stream.as_ref()?.placement.as_ref()?.key)
}

/// `stream` as a session for ducking; `None` without an identifier for the journal.
fn session(stream: &Stream) -> Option<NodeSession<'_>> {
    Some(NodeSession {
        id: stream.serial.clone(),
        identifier: stream.identifier.clone()?,
        node: &stream.proxy,
        volume: &stream.volume,
    })
}
