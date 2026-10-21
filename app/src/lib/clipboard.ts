/**
 * Copies `text` to the clipboard. Returns whether it worked.
 *
 * The clipboard API is only there on pages the browser trusts, and the app's own pages are not always counted as such
 * (WebKitGTK is picky), so when it is missing or refuses, the text goes in a hidden field that is selected and copied
 * the old way.
 */
export async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // Try the other way.
  }
  const field = document.createElement('textarea');
  field.value = text;
  field.setAttribute('readonly', '');
  field.style.position = 'fixed';
  field.style.opacity = '0';
  document.body.append(field);
  try {
    field.focus();
    field.select();
    return document.execCommand('copy');
  } catch {
    return false;
  } finally {
    field.remove();
  }
}
