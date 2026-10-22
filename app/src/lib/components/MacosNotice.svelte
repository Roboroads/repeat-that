<script lang="ts">
  import { openLink } from '../opener';
  import { isMac } from '../platform';

  // The macOS issue form. The opener capability allows this project's GitHub pages.
  const MACOS_REPORT_URL = 'https://github.com/Roboroads/repeat-that/issues/new?template=macos.yml';

  const mac = isMac();
</script>

<!-- Shown until someone has confirmed that it works on a real Mac. -->
{#if mac}
  <p class="callout">
    macOS support is new and hasn't been tested on a real Mac yet. If something doesn't work, please
    <a href={MACOS_REPORT_URL} target="_blank" rel="noreferrer" onclick={(event) => openLink(event, MACOS_REPORT_URL)}>report it</a>.
  </p>
{/if}

<style>
  a {
    color: inherit;
  }
</style>
