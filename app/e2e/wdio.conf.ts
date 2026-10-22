// End-to-end test of the desktop app: WebdriverIO drives the real debug build through tauri-driver, which starts
// WebKitWebDriver and the app. Linux only, because the Linux build is the one that can run on a virtual display. The app
// doesn't capture real apps here: it plays a speech recording as a fake app (`speech_8s`, see `REPEAT_THAT_MOCK_WAV`
// below) and replays into nothing.
//
// Run it with `npm run test:e2e` (on a machine without a display: `xvfb-run -a npm run test:e2e`), which builds the
// debug app first (the `pretest:e2e` script). Needs Node.js 22.19 or later, `tauri-driver` (`cargo install tauri-driver
// --locked`) and `WebKitWebDriver` (Debian/Ubuntu: the `webkit2gtk-driver` package). More in CONTRIBUTING.md.
import { type ChildProcess, spawn, spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdir, mkdtemp, rm } from 'node:fs/promises';
import net from 'node:net';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const appDir = path.resolve(import.meta.dirname, '..');
const repoDir = path.resolve(appDir, '..');
const targetDir = process.env.CARGO_TARGET_DIR
  ? path.resolve(process.env.CARGO_TARGET_DIR)
  : path.join(repoDir, 'target');
const application = path.join(targetDir, 'debug', 'repeat-that');
/** The speech recording the app plays as its only app, `speech_8s`. */
const fixture = path.join(repoDir, 'crates', 'rt-core', 'tests', 'fixtures', 'speech_8s.wav');

const driverCommand = process.env.TAURI_DRIVER ?? 'tauri-driver';

/**
 * Variables some spec files start their app with, by file name. `permission.e2e.ts` gets an app that pretends the OS
 * doesn't let it capture audio until "Ask again" (debug builds only, see src-tauri/src/source.rs). The update specs
 * each get a pretend update source (`REPEAT_THAT_MOCK_UPDATE`, debug builds only, see src-tauri/src/updater/source.rs):
 * an app has one for its whole run, so every answer it can give has a spec file. `updates-off.e2e.ts` is not in the
 * list on purpose, since a build without the variable has no updater, and neither has any other spec's app.
 */
const SPEC_ENV: Record<string, NodeJS.ProcessEnv> = {
  'permission.e2e.ts': { REPEAT_THAT_MOCK_PERMISSION: 'denied' },
  'updates.e2e.ts': { REPEAT_THAT_MOCK_UPDATE: 'available' },
  'updates-fail-download.e2e.ts': { REPEAT_THAT_MOCK_UPDATE: 'fail-download' },
  'updates-fail-check.e2e.ts': { REPEAT_THAT_MOCK_UPDATE: 'fail-check' },
  'updates-none.e2e.ts': { REPEAT_THAT_MOCK_UPDATE: 'none' },
};

/** The variables of `SPEC_ENV` for a spec, which WebdriverIO names by a `file://` URL or by a path. */
function specEnv(spec: string): NodeJS.ProcessEnv | undefined {
  const file = spec.startsWith('file:') ? fileURLToPath(spec) : spec;
  return SPEC_ENV[path.basename(file)];
}

/** What one session runs: a private D-Bus, a tauri-driver (with the app below it) and the app's folders. */
interface Session {
  root: string;
  bus: ChildProcess;
  driver: ChildProcess;
}
let session: Session | null = null;

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** A port nobody listens on right now. */
function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address() as net.AddressInfo;
      server.close(() => resolve(port));
    });
  });
}

/** Stops a process and everything it started (it leads its own process group). */
async function stop(child: ChildProcess): Promise<void> {
  if (child.pid === undefined) return;
  const exited = new Promise((resolve) => child.once('exit', resolve));
  for (const signal of ['SIGTERM', 'SIGKILL'] as const) {
    try {
      process.kill(-child.pid, signal);
    } catch {
      return; // The whole group is gone.
    }
    await Promise.race([exited, sleep(3_000)]);
  }
}

/** Kills the groups of a session synchronously: for when the worker ends without `afterSession` having run. */
function killNow({ bus, driver }: Session): void {
  for (const child of [driver, bus]) {
    try {
      if (child.pid !== undefined) process.kill(-child.pid, 'SIGKILL');
    } catch {
      // Already gone.
    }
  }
}

/**
 * Waits until `ready` says yes while `child` is still running. The answer only counts when `child` is still there a
 * moment later: a driver that finds its port taken exits at once, and then whoever else holds the port (a driver left
 * over from an earlier run, say) must not be mistaken for it.
 */
async function waitFor(what: string, ready: () => Promise<boolean>, child: ChildProcess): Promise<void> {
  const deadline = Date.now() + 20_000;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) throw new Error(`${what} exited with code ${child.exitCode}`);
    if (await ready().catch(() => false)) {
      await sleep(300);
      if (child.exitCode !== null) throw new Error(`${what} exited with code ${child.exitCode} right after it came up`);
      return;
    }
    await sleep(100);
  }
  throw new Error(`${what} did not come up`);
}

export const config: WebdriverIO.Config = {
  runner: 'local',
  specs: ['./specs/**/*.e2e.ts'],
  // Every spec file gets its own app, with fresh settings (see `beforeSession`), and one app at a time: the global
  // hotkeys and the audio engine are not meant to be shared.
  maxInstances: 1,
  hostname: '127.0.0.1',
  // A placeholder: each session gets a driver on a free port of its own, which `beforeSession` puts here.
  port: 4444,
  capabilities: [{ maxInstances: 1, 'tauri:options': { application } } as WebdriverIO.Capabilities],
  logLevel: 'warn',
  waitforTimeout: 15_000,
  connectionRetryTimeout: 60_000,
  connectionRetryCount: 0,
  framework: 'mocha',
  reporters: ['spec'],
  mochaOpts: { ui: 'bdd', timeout: 90_000 },

  onPrepare() {
    const missing = spawnSync(driverCommand, ['--help'], { stdio: 'ignore' }).error;
    if (missing) {
      throw new Error(`${driverCommand} is not installed (cargo install tauri-driver --locked): ${missing.message}`);
    }
  },

  /**
   * Starts what one app needs, so that each spec file gets an app that has never run before and cannot meet another
   * one:
   *  - One scratch folder for everything the app can write to by default: its home, temp, runtime, config, data and
   *    cache folders. Only settings are meant to end up there, never audio, and the privacy spec looks through it.
   *  - Its own D-Bus session. The single-instance check of the app goes over the session bus, so an app already running
   *    on this machine (the real one, or a stuck test run) would swallow the test's launch, and a CI runner may have no
   *    session bus at all.
   *  - Its own tauri-driver and WebKitWebDriver, on ports that are free right now. A driver left over from an earlier
   *    run on a fixed port would answer instead, and start the app with the real settings and D-Bus.
   *  - No GPU. WebKit asks the GPU's DRM device for the display's refresh rate otherwise, and on a virtual GPU that
   *    call can hang for good, with the app stuck in the kernel where nothing can kill it. Software rendering is also
   *    what a CI runner has, so the test runs the same everywhere.
   *  - The speech fixture as the only app (`REPEAT_THAT_MOCK_WAV`), instead of the PipeWire backend a Linux build
   *    has, and an output that plays into nothing (`REPEAT_THAT_NULL_OUTPUT`). See src-tauri/src/source.rs.
   * A spec file in `SPEC_ENV` gets its variables on top; the others never see them, even when they are set here.
   */
  async beforeSession(config, _capabilities, specs) {
    const root = await mkdtemp(path.join(tmpdir(), 'repeat-that-e2e-'));
    const names = ['home', 'tmp', 'run', 'config', 'data', 'cache'];
    const folders = Object.fromEntries(names.map((name) => [name, path.join(root, name)]));
    await Promise.all(Object.values(folders).map((folder) => mkdir(folder, { mode: 0o700 })));
    const busSocket = path.join(root, 'bus');
    const bus = spawn('dbus-daemon', ['--session', '--nofork', `--address=unix:path=${busSocket}`], {
      detached: true,
      stdio: 'ignore',
    });
    const port = await freePort();
    let nativePort = await freePort();
    while (nativePort === port) nativePort = await freePort();
    const env: NodeJS.ProcessEnv = {
      ...process.env,
      HOME: folders.home,
      TMPDIR: folders.tmp,
      XDG_RUNTIME_DIR: folders.run,
      XDG_CONFIG_HOME: folders.config,
      XDG_DATA_HOME: folders.data,
      XDG_CACHE_HOME: folders.cache,
      DBUS_SESSION_BUS_ADDRESS: `unix:path=${busSocket}`,
      WEBKIT_FORCE_VBLANK_TIMER: '1',
      WEBKIT_DISABLE_DMABUF_RENDERER: '1',
      WEBKIT_DISABLE_COMPOSITING_MODE: '1',
      LIBGL_ALWAYS_SOFTWARE: '1',
      REPEAT_THAT_NULL_OUTPUT: '1',
      REPEAT_THAT_MOCK_WAV: fixture,
    };
    for (const own of Object.values(SPEC_ENV)) for (const name of Object.keys(own)) delete env[name];
    for (const spec of specs) Object.assign(env, specEnv(spec));
    const driver = spawn(driverCommand, ['--port', String(port), '--native-port', String(nativePort)], {
      env,
      detached: true,
      stdio: ['ignore', 'inherit', 'inherit'],
    });
    session = { root, bus, driver };
    // For the specs, which run in this process: they look in it for what the app left behind.
    process.env.E2E_APP_HOME = root;
    // A session that fails to start never reaches `afterSession`, and must not leave a driver behind.
    process.once('exit', () => session && killNow(session));
    await waitFor('dbus-daemon', async () => existsSync(busSocket), bus);
    await waitFor(driverCommand, async () => (await fetch(`http://127.0.0.1:${port}/status`)).ok, driver);
    config.port = port;
  },

  async afterSession() {
    if (session === null) return;
    const { root, bus, driver } = session;
    session = null;
    await stop(driver);
    await stop(bus);
    await rm(root, { recursive: true, force: true });
  },
};
