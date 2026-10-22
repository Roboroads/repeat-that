import { existsSync } from "node:fs";
import { join } from "node:path";
import { MEDIA_FILES, mediaDirectory } from "../../integrations/media.mjs";

export interface Media {
  /** Where the page finds the files. They are on the site's own domain. */
  video: string;
  poster?: string;
  captions?: string;
}

export function mediaDir(env: Record<string, string | undefined> = process.env, root: string = process.cwd()): string {
  return mediaDirectory(env, root);
}

/**
 * The demo video, or null when there isn't one yet. The deploy workflow puts `demo.mp4` (and a poster and captions if
 * the release has them) in `public/media`. No video, no video section.
 */
export function findMedia(dir: string): Media | null {
  if (!existsSync(join(dir, MEDIA_FILES.video))) return null;
  const media: Media = { video: `/media/${MEDIA_FILES.video}` };
  if (existsSync(join(dir, MEDIA_FILES.poster))) media.poster = `/media/${MEDIA_FILES.poster}`;
  if (existsSync(join(dir, MEDIA_FILES.captions))) media.captions = `/media/${MEDIA_FILES.captions}`;
  return media;
}
