import * as api from './api';
import type { AppEntry, OutputDevice, Settings, SettingsPatch, Status, UpdateStatus } from './api';
import { offUpdateStatus } from './defaults';

const APP_REFRESH_MS = 2000;

/**
 * A change to the settings. A function is given the settings as they are when the change is sent, with every earlier
 * change already in, so a change to part of a nested object (`duck`, `hotkeys`) cannot undo one before it.
 */
export type SettingsChange = SettingsPatch | ((current: Settings) => SettingsPatch);

const resolve = (change: SettingsChange, current: Settings): SettingsPatch =>
  typeof change === 'function' ? change(current) : change;

interface UpdateOptions {
  /** Show the new value before the backend confirmed it. Default true. */
  optimistic?: boolean;
  /** Put a failure in the page's error banner. Default true; a caller that shows the error itself passes false. */
  report?: boolean;
}

/**
 * What the settings page knows: the backend's settings and status, the update status, the running apps and the output
 * devices. Every change goes to the backend at once (`update`); there is no Save button.
 */
export class SettingsStore {
  settings = $state<Settings | null>(null);
  status = $state<Status | null>(null);
  apps = $state<AppEntry[]>([]);
  appsLoaded = $state(false);
  /** Why the last app list refresh failed; `null` after a good one. */
  appsError = $state<string | null>(null);
  devices = $state<OutputDevice[]>([]);
  devicesLoaded = $state(false);
  /** The backend did not answer the first load. */
  loadError = $state<string | null>(null);
  /** The last failed action, shown in a banner until dismissed. */
  error = $state<string | null>(null);
  /** What the build does about updates. A build or backend without an updater is mode `off`. */
  updates = $state<UpdateStatus>(offUpdateStatus());
  /** The version whose banner the user closed with "Later". A newer version is offered again. */
  private dismissedVersion = $state<string | null>(null);
  /** A check or install request is out, so its button can't be pressed twice. */
  updateCallPending = $state(false);
  /**
   * How many `update-status` events came in. The backend sends one for every change, so the last one is the truth; a
   * snapshot or an answer that was asked for before an event came in can be older than it.
   */
  private updateEvents = 0;

  private unlisteners: api.Unlisten[] = [];
  private disposed = false;
  // Everything that changes settings goes through one queue, one call at a time: an answer can then never overwrite
  // a change that was made after the call it answers.
  private inFlight = 0;
  private queue: Promise<unknown> = Promise.resolve();
  /** The settings as the backend last confirmed them. `settings` is this plus the changes still in the queue. */
  private confirmed: Settings | null = null;
  /** The queue's last call failed: what is shown may be wrong, fetch the truth once the queue is empty. */
  private needsResync = false;
  private deviceNames = new Map<string, string>();
  private appsBusy = false;
  private devicesBusy = false;
  private pollTimer: ReturnType<typeof setInterval> | null = null;

  /** Loads everything, subscribes to the backend's events, and starts refreshing the app list while visible. */
  async start(): Promise<void> {
    this.disposed = false;
    document.addEventListener('visibilitychange', this.onVisibility);
    await this.load();
  }

  /**
   * (Re)subscribes if that has not worked yet, then (re)loads settings and status; the apps and devices follow
   * without holding the page back. Also what "Try again" runs.
   */
  async load(): Promise<void> {
    this.loadError = null;
    try {
      await this.subscribe();
      const eventsBefore = this.updateEvents;
      const [settings, status, updates] = await Promise.all([api.getSettings(), api.getStatus(), this.loadUpdates()]);
      if (this.disposed) return;
      this.settings = this.confirmed = settings;
      this.status = status;
      // An event that came in while the snapshot was on its way is newer than it.
      if (this.updateEvents === eventsBefore) this.updates = updates;
    } catch (raw) {
      if (!this.disposed) this.loadError = api.toApiError(raw).message;
      return;
    }
    if (this.isVisible()) this.startPolling();
    void this.refreshDevices();
    await this.refreshApps();
  }

  /** A backend without the update commands (an older one) answers an error here: that is a build without updates. */
  private async loadUpdates(): Promise<UpdateStatus> {
    try {
      return await api.getUpdateStatus();
    } catch {
      return offUpdateStatus();
    }
  }

  /**
   * Listens to `status`, `update-status` and `settings-changed`. Does nothing when it already does; throws when it
   * cannot.
   */
  private async subscribe(): Promise<void> {
    if (this.unlisteners.length > 0) return;
    const unlisteners: api.Unlisten[] = [];
    try {
      unlisteners.push(
        await api.onStatus((status) => {
          this.status = status;
        }),
      );
      unlisteners.push(
        await api.onUpdateStatus((updates) => {
          this.updateEvents++;
          this.updates = updates;
        }),
      );
      unlisteners.push(
        await api.onSettingsChanged((settings) => {
          if (this.inFlight === 0) {
            this.settings = this.confirmed = settings;
          } else {
            // Our own calls are out, and their answers may predate this change: fetch the truth when they are done.
            this.needsResync = true;
          }
        }),
      );
    } catch (error) {
      unlisteners.forEach((unlisten) => unlisten());
      throw error;
    }
    if (this.disposed) {
      unlisteners.forEach((unlisten) => unlisten());
      return;
    }
    this.unlisteners = unlisteners;
  }

  dispose(): void {
    this.disposed = true;
    this.unlisteners.forEach((unlisten) => unlisten());
    this.unlisteners = [];
    this.stopPolling();
    document.removeEventListener('visibilitychange', this.onVisibility);
  }

  /**
   * Saves a change. Resolves with the backend's refusal, or `null` when it went through. The page shows the new
   * value at once and takes the backend's answer when the queue is empty, or the old value back when it is refused.
   */
  async update(change: SettingsChange, options: UpdateOptions = {}): Promise<api.ApiError | null> {
    const { optimistic = true, report = true } = options;
    if (this.settings === null) {
      // Nothing to change yet. Answering "saved" would let a caller go on as if it had worked.
      const error = new api.ApiError('backend', 'The settings are not loaded yet.');
      if (report) this.error = error.message;
      return error;
    }
    const shown = this.settings;
    if (optimistic) this.settings = { ...shown, ...resolve(change, shown) };

    return this.enqueue(async () => {
      try {
        // Built only now: the calls before this one are answered, so `confirmed` has what they changed.
        this.confirmed = await api.updateSettings(resolve(change, this.confirmed ?? shown));
        return null;
      } catch (raw) {
        const error = api.toApiError(raw);
        if (report) this.error = error.message;
        this.needsResync = true;
        return error;
      }
    });
  }

  /** Makes `app` the target. The backend wipes its buffer; picking one also ends the first run. */
  async selectApp(app: AppEntry): Promise<void> {
    if (this.settings === null) return;
    const firstRun = !this.settings.firstRunDone;
    this.settings = { ...this.settings, target: { exePath: app.exePath, name: app.name } };
    await this.enqueue(async () => {
      try {
        await api.selectApp(app);
        this.confirmed = firstRun ? await api.updateSettings({ firstRunDone: true }) : await api.getSettings();
      } catch (raw) {
        this.error = api.toApiError(raw).message;
        this.needsResync = true;
      }
    });
  }

  /** Runs `call` after everything queued before it, and settles the page once the queue is empty. */
  private enqueue<T>(call: () => Promise<T>): Promise<T> {
    this.inFlight++;
    const run = this.queue.then(call);
    this.queue = run.then(
      () => undefined,
      () => undefined,
    );
    return run.finally(() => {
      this.inFlight--;
      if (this.inFlight === 0) void this.settle();
    });
  }

  /** The queue is empty: show what the backend confirmed, or fetch it again after a failure. */
  private async settle(): Promise<void> {
    if (this.disposed) return;
    if (this.needsResync) {
      this.needsResync = false;
      await this.resync();
    } else if (this.confirmed !== null) {
      this.settings = this.confirmed;
    }
  }

  async replay(): Promise<void> {
    await this.run(api.replay);
  }

  async stopPlayback(): Promise<void> {
    await this.run(api.stop);
  }

  async setPaused(paused: boolean): Promise<void> {
    await this.run(() => api.setPaused(paused));
  }

  /** Looks for a newer version now, as the user asked, and shows the offer again if they had closed it. */
  async checkForUpdates(): Promise<void> {
    this.dismissedVersion = null;
    if (this.updateCallPending) return;
    this.updateCallPending = true;
    const eventsBefore = this.updateEvents;
    try {
      const answer = await api.checkForUpdates();
      // The answer is what the backend knew when it replied; an event that came in meanwhile is newer than that.
      if (this.updateEvents === eventsBefore) this.updates = answer;
    } catch (raw) {
      this.error = api.toApiError(raw).message;
    } finally {
      this.updateCallPending = false;
    }
  }

  /** Downloads and installs the version found. The progress and the restart come as `update-status` events. */
  async installUpdate(): Promise<void> {
    if (this.updateCallPending) return;
    this.updateCallPending = true;
    const eventsBefore = this.updateEvents;
    try {
      await api.installUpdate();
      // The backend took it. Its event can be behind this answer, and until it lands the page would offer the same
      // install again.
      if (this.updateEvents === eventsBefore && this.updates.state === 'available') {
        this.updates = { ...this.updates, state: 'downloading', progress: null, error: null };
      }
    } catch (raw) {
      this.error = api.toApiError(raw).message;
    } finally {
      this.updateCallPending = false;
    }
  }

  /** "Later": hides the offer of the version found until the next launch, or until a newer one turns up. */
  dismissUpdate(): void {
    this.dismissedVersion = this.updates.available?.version ?? null;
  }

  showUpdate(): void {
    this.dismissedVersion = null;
  }

  /** The user closed the banner of the version on offer. */
  get offerDismissed(): boolean {
    const offer = this.updates.available;
    return offer !== null && offer.version === this.dismissedVersion;
  }

  async refreshApps(): Promise<void> {
    if (this.appsBusy) return;
    this.appsBusy = true;
    try {
      const apps = await api.listApps();
      if (this.disposed) return;
      this.apps = apps;
      this.appsLoaded = true;
      this.appsError = null;
    } catch (raw) {
      // Keep the list we have; the next refresh tries again.
      if (!this.disposed) this.appsError = api.toApiError(raw).message;
    } finally {
      this.appsBusy = false;
    }
  }

  async refreshDevices(): Promise<void> {
    if (this.devicesBusy) return;
    this.devicesBusy = true;
    try {
      const devices = await api.listOutputDevices();
      if (this.disposed) return;
      for (const device of devices) this.deviceNames.set(device.id, device.name);
      this.devices = devices;
      this.devicesLoaded = true;
    } catch {
      // The dropdown keeps what it has.
    } finally {
      this.devicesBusy = false;
    }
  }

  /** The name to show for a device id: the one seen this session, else the id itself. */
  deviceName(id: string): string {
    return this.deviceNames.get(id) ?? id;
  }

  dismissError(): void {
    this.error = null;
  }

  private async resync(): Promise<void> {
    try {
      const settings = await api.getSettings();
      if (this.disposed) return;
      this.confirmed = settings;
      // A change made while this was on its way is not in `settings` yet: leave the display to the queue then.
      if (this.inFlight === 0) this.settings = settings;
    } catch {
      // The backend is not answering; what we show stays.
    }
  }

  private async run(action: () => Promise<unknown>): Promise<void> {
    try {
      await action();
    } catch (raw) {
      this.error = api.toApiError(raw).message;
    }
  }

  private isVisible(): boolean {
    return document.visibilityState !== 'hidden';
  }

  private startPolling(): void {
    if (this.pollTimer !== null || this.disposed) return;
    this.pollTimer = setInterval(() => void this.refreshApps(), APP_REFRESH_MS);
  }

  private stopPolling(): void {
    if (this.pollTimer === null) return;
    clearInterval(this.pollTimer);
    this.pollTimer = null;
  }

  private onVisibility = (): void => {
    if (this.settings === null) return;
    if (this.isVisible()) {
      void this.refreshApps();
      void this.refreshDevices();
      this.startPolling();
    } else {
      this.stopPolling();
    }
  };
}
