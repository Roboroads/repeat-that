// End-to-end test of the desktop app: WebdriverIO drives the real debug build through tauri-driver, which starts
// WebKitWebDriver and the app. Linux only, because the Linux build is the one that can run without a real capture
// backend: it plays a speech recording as a fake app (`speech_8s`) and replays into nothing.
//
// Run it with `npm run test:e2e` (on a machine without a display: `xvfb-run -a npm run test:e2e`), which builds the
// debug app first (the `pretest:e2e` script). Needs `tauri-driver` (`cargo install tauri-driver --locked`) and
// `WebKitWebDriver` (Debian/Ubuntu: the `webkit2gtk-driver` package). More in CONTRIBUTING.md.
import { type ChildProcess, spawn, spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdir, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';

const appDir = path.resolve(import.meta.dirname, '..');
const repoDir = path.resolve(appDir, '..');
const targetDir = process.env.CARGO_TARGET_DIR
  ? path.resolve(process.env.CARGO_TARGET_DIR)
  : path.join(repoDir, 'target');
const application = path.join(targetDir, 'debug', 'repeat-that');

const driverCommand = process.env.TAURI_DRIVER ?? 'tauri-driver';
const driverPort = 4444;

/** What one session runs: a private D-Bus, a tauri-driver (with the app below it) and the app's folders. */
interface Session {
  home: string;
  bus: ChildProcess;
  driver: ChildProcess;
}
let session: Session | null = null;

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
    await Promise.race([exited, new Promise((resolve) => setTimeout(resolve, 3_000))]);
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

async function waitFor(what: string, ready: () => Promise<boolean>, child: ChildProcess): Promise<void> {
  const deadline = Date.now() + 20_000;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) throw new Error(`${what} exited with code ${child.exitCode}`);
    if (await ready().catch(() => false)) return;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`${what} did not come up`);
}

export const config: WebdriverIO.Config = {
  runner: 'local',
  specs: ['./specs/**/*.e2e.ts'],
  // Every spec file gets its own app, with fresh settings (see `beforeSession`), and one app at a time: the app
  // allows one running copy, and the global hotkeys and the audio engine are not meant to be shared.
  maxInstances: 1,
  hostname: '127.0.0.1',
  port: driverPort,
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
   *  - Its own settings folder (the app reads XDG_CONFIG_HOME). Only settings are written there, never audio.
   *  - Its own D-Bus session. The single-instance check of the app goes over the session bus, so an app already running
   *    on this machine (the real one, or a stuck test run) would swallow the test's launch, and a CI runner may have no
   *    session bus at all.
   *  - No GPU. WebKit asks the GPU's DRM device for the display's refresh rate otherwise, and on a virtual GPU that
   *    call can hang for good, with the app stuck in the kernel where nothing can kill it. Software rendering is also
   *    what a CI runner has, so the test runs the same everywhere.
   * The null output and the fixture mock are what a debug build uses on Linux anyway (see src-tauri/src/source.rs).
   */
  async beforeSession() {
    const home = await mkdtemp(path.join(tmpdir(), 'repeat-that-e2e-'));
    await Promise.all(['config', 'data', 'cache'].map((name) => mkdir(path.join(home, name))));
    const busSocket = path.join(home, 'bus');
    const bus = spawn('dbus-daemon', ['--session', '--nofork', `--address=unix:path=${busSocket}`], {
      detached: true,
      stdio: 'ignore',
    });
    const env: NodeJS.ProcessEnv = {
      ...process.env,
      DBUS_SESSION_BUS_ADDRESS: `unix:path=${busSocket}`,
      XDG_CONFIG_HOME: path.join(home, 'config'),
      XDG_DATA_HOME: path.join(home, 'data'),
      XDG_CACHE_HOME: path.join(home, 'cache'),
      WEBKIT_FORCE_VBLANK_TIMER: '1',
      WEBKIT_DISABLE_DMABUF_RENDERER: '1',
      WEBKIT_DISABLE_COMPOSITING_MODE: '1',
      LIBGL_ALWAYS_SOFTWARE: '1',
      REPEAT_THAT_NULL_OUTPUT: '1',
    };
    delete env.REPEAT_THAT_MOCK_WAV;
    const driver = spawn(driverCommand, ['--port', String(driverPort)], {
      env,
      detached: true,
      stdio: ['ignore', 'inherit', 'inherit'],
    });
    session = { home, bus, driver };
    // For the specs, which run in this process: they look in it for what the app left behind.
    process.env.E2E_APP_HOME = home;
    // A session that fails to start never reaches `afterSession`, and must not leave a driver on the port.
    process.once('exit', () => session && killNow(session));
    await waitFor('dbus-daemon', async () => existsSync(busSocket), bus);
    await waitFor(
      driverCommand,
      async () => (await fetch(`http://127.0.0.1:${driverPort}/status`)).ok,
      driver,
    );
  },

  async afterSession() {
    if (session === null) return;
    const { home, bus, driver } = session;
    session = null;
    await stop(driver);
    await stop(bus);
    await rm(home, { recursive: true, force: true });
  },
};
