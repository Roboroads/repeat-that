import { readFileSync } from "node:fs";
import { resolve } from "node:path";

/** The JSON `.github/scripts/site_release.py` writes. Bump `schema` on both sides together. */
export const SCHEMA = 1;

export const KINDS = ["setup-exe", "dmg", "appimage", "deb", "rpm", "flatpak"] as const;
export type Kind = (typeof KINDS)[number];

export interface ReleaseFile {
  name: string;
  kind: Kind;
  url: string;
  size: number;
  sha256: string;
}

export interface ReleasePlatform {
  /** `windows`, `linux`, `flatpak` or `macos`. The data decides which of them exist. */
  name: string;
  label: string;
  files: ReleaseFile[];
}

export interface Release {
  tag: string;
  version: string;
  published_at: string;
  url: string;
  sums_url: string;
  platforms: ReleasePlatform[];
}

export interface ReleaseData {
  schema: typeof SCHEMA;
  release: Release | null;
}

class ReleaseDataError extends Error {}

type Obj = Record<string, unknown>;

function object(value: unknown, path: string): Obj {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new ReleaseDataError(`${path} must be an object`);
  }
  return value as Obj;
}

function text(obj: Obj, key: string, path: string): string {
  const value = obj[key];
  if (typeof value !== "string" || value === "") throw new ReleaseDataError(`${path}.${key} must be a non-empty string`);
  return value;
}

function httpsUrl(obj: Obj, key: string, path: string): string {
  const value = text(obj, key, path);
  if (!value.startsWith("https://")) throw new ReleaseDataError(`${path}.${key} must be an https url`);
  return value;
}

function file(raw: unknown, path: string): ReleaseFile {
  const obj = object(raw, path);
  const kind = text(obj, "kind", path);
  if (!(KINDS as readonly string[]).includes(kind)) {
    throw new ReleaseDataError(`${path}.kind is "${kind}", expected one of ${KINDS.join(", ")}`);
  }
  const size = obj.size;
  if (typeof size !== "number" || !Number.isInteger(size) || size < 0) {
    throw new ReleaseDataError(`${path}.size must be a whole number of bytes`);
  }
  const sha256 = text(obj, "sha256", path);
  if (!/^[0-9a-f]{64}$/.test(sha256)) throw new ReleaseDataError(`${path}.sha256 must be 64 lowercase hex digits`);
  return { name: text(obj, "name", path), kind: kind as Kind, url: httpsUrl(obj, "url", path), size, sha256 };
}

function platform(raw: unknown, path: string): ReleasePlatform {
  const obj = object(raw, path);
  if (!Array.isArray(obj.files) || obj.files.length === 0) throw new ReleaseDataError(`${path}.files must not be empty`);
  return {
    name: text(obj, "name", path),
    label: text(obj, "label", path),
    files: obj.files.map((f, i) => file(f, `${path}.files[${i}]`)),
  };
}

/** Checks the release data and returns it typed. Throws with the path of the first thing that is wrong. */
export function parseRelease(raw: unknown): ReleaseData {
  const root = object(raw, "release data");
  if (root.schema !== SCHEMA) {
    throw new ReleaseDataError(`release data has schema ${JSON.stringify(root.schema)}, this site reads schema ${SCHEMA}`);
  }
  if (!("release" in root)) throw new ReleaseDataError("release data has no release key (use null when there is none)");
  if (root.release === null) return { schema: SCHEMA, release: null };

  const obj = object(root.release, "release");
  if (!Array.isArray(obj.platforms)) throw new ReleaseDataError("release.platforms must be a list");
  return {
    schema: SCHEMA,
    release: {
      tag: text(obj, "tag", "release"),
      version: text(obj, "version", "release"),
      published_at: text(obj, "published_at", "release"),
      url: httpsUrl(obj, "url", "release"),
      sums_url: httpsUrl(obj, "sums_url", "release"),
      platforms: obj.platforms.map((p, i) => platform(p, `release.platforms[${i}]`)),
    },
  };
}

/**
 * Reads the release data the build was given: the file `SITE_RELEASE_JSON` names (relative to the site folder), or the
 * committed sample. A file that doesn't pass `parseRelease` fails the build.
 */
export function loadRelease(env: Record<string, string | undefined> = process.env, cwd: string = process.cwd()): ReleaseData {
  const configured = env.SITE_RELEASE_JSON;
  const path = configured ? resolve(cwd, configured) : resolve(cwd, "src/data/release.sample.json");
  let raw: unknown;
  try {
    raw = JSON.parse(readFileSync(path, "utf8"));
  } catch (error) {
    throw new ReleaseDataError(`cannot read the release data from ${path} (SITE_RELEASE_JSON): ${(error as Error).message}`);
  }
  try {
    return parseRelease(raw);
  } catch (error) {
    throw new ReleaseDataError(`${path}: ${(error as Error).message}`);
  }
}

/** The file the download button of a platform points at. */
const PREFERRED: Record<string, Kind[]> = {
  windows: ["setup-exe"],
  macos: ["dmg"],
  linux: ["appimage", "deb", "rpm"],
  flatpak: ["flatpak"],
};

export function primaryFile(platform: ReleasePlatform): ReleaseFile {
  for (const kind of PREFERRED[platform.name] ?? []) {
    const match = platform.files.find((f) => f.kind === kind);
    if (match) return match;
  }
  return platform.files[0];
}

const KIND_LABELS: Record<Kind, string> = {
  "setup-exe": "Installer (.exe)",
  dmg: "Disk image (.dmg)",
  appimage: "AppImage, any distribution",
  deb: "Debian and Ubuntu (.deb)",
  rpm: "Fedora (.rpm)",
  flatpak: "Flatpak bundle",
};

export function kindLabel(kind: Kind): string {
  return KIND_LABELS[kind];
}

/** File sizes the way download pages show them: decimal units, one decimal for megabytes. */
export function formatSize(bytes: number): string {
  if (bytes >= 1_000_000) return `${(bytes / 1_000_000).toFixed(1)} MB`;
  if (bytes >= 1_000) return `${Math.round(bytes / 1_000)} kB`;
  return `${bytes} B`;
}

const MONTHS = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
];

/** "21 October 2026". Fixed English text in UTC, so a build prints the same date on every machine. */
export function formatDate(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return `${date.getUTCDate()} ${MONTHS[date.getUTCMonth()]} ${date.getUTCFullYear()}`;
}
