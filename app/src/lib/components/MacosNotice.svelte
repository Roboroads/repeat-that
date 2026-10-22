<script lang="ts">
  import { openExternal } from '../opener';
  import { isMac } from '../platform';

  // The macOS issue form. The opener capability allows this project's GitHub pages.
  const MACOS_REPORT_URL = 'https://github.com/Roboroads/repeat-that/issues/new?template=macos.yml';

  const mac = isMac();

  // A plain link does nothing inside the app window; see openExternal.
  function report(event: MouseEvent) {
    event.preventDefault();
    void openExternal(MACOS_REPORT_URL);
  }
</script>

<!-- Shown until someone has confirmed that it works on a real Mac. -->
{#if mac}
  <p class="callout">
    macOS support is new and hasn't been tested on a real Mac yet. If something doesn't work, please
    <a href={MACOS_REPORT_URL} target="_blank" rel="noreferrer" onclick={report}>report it</a>.
  </p>
{/if}

<style>
  a {
    color: inherit;
  }
</style>
