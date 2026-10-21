import type { AppEntry, Settings } from './api';

type Target = Settings['target'];

/**
 * Whether `app` is the remembered target. Like the engine: by exe path when the target has one, by name when it has
 * not (the mock source). Case-insensitive, since Windows paths are.
 */
export function matchesTarget(target: Target, app: AppEntry): boolean {
  if (target === null) return false;
  if (target.exePath !== null) {
    return app.exePath !== null && app.exePath.toLowerCase() === target.exePath.toLowerCase();
  }
  return app.name.toLowerCase() === target.name.toLowerCase();
}

export interface AppRow {
  key: string;
  name: string;
  icon: string | null;
  selected: boolean;
  /** The remembered target is not running: shown pinned on top as "<name> · waiting". */
  waiting: boolean;
  /** `null` for the pinned row of a target that is not running. */
  app: AppEntry | null;
}

function identity(app: AppEntry): string {
  return app.exePath !== null ? `path:${app.exePath.toLowerCase()}` : `name:${app.name.toLowerCase()}`;
}

/** The rows of the app list: one per exe (the first, as the engine picks), the target pinned on top if it is gone. */
export function appRows(apps: AppEntry[], target: Target): AppRow[] {
  const seen = new Set<string>();
  const rows: AppRow[] = [];
  for (const app of apps) {
    const id = identity(app);
    if (seen.has(id)) continue;
    seen.add(id);
    rows.push({ key: id, name: app.name, icon: app.icon, selected: matchesTarget(target, app), waiting: false, app });
  }
  if (target !== null && !rows.some((row) => row.selected)) {
    rows.unshift({ key: 'target', name: target.name, icon: null, selected: true, waiting: true, app: null });
  }
  return rows;
}
