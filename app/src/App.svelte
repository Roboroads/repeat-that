<script lang="ts">
  // The app icon's source file: the same SVG the PNG and ICO app icons are rendered from.
  import icon from '../src-tauri/icons/svg/app.svg';
</script>

<main>
  <img class="logo" src={icon} alt="" width="72" height="72" />
  <h1>Repeat That</h1>
  <p>Settings are coming soon.</p>
  <p class="hint">Closing this window keeps the app running in the tray. Use the tray menu to quit.</p>
</main>

<style>
  main {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.5rem;
    padding: 3rem 1.5rem;
    text-align: center;
  }

  h1 {
    margin: 0.5rem 0 0;
    font-size: 1.75rem;
  }

  p {
    margin: 0;
  }

  .hint {
    color: var(--text-muted);
    max-width: 24rem;
  }
</style>
