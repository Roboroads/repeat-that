<script lang="ts">
  interface Props {
    label: string;
    description?: string;
    checked: boolean;
    onchange: (checked: boolean) => void;
    disabled?: boolean;
  }

  let { label, description, checked, onchange, disabled = false }: Props = $props();
  const uid = $props.id();
</script>

<div class="switch">
  <div class="text">
    <label for="{uid}-input" id="{uid}-label">{label}</label>
    {#if description}<p id="{uid}-desc" class="muted">{description}</p>{/if}
  </div>
  <input
    id="{uid}-input"
    type="checkbox"
    role="switch"
    {checked}
    {disabled}
    aria-labelledby="{uid}-label"
    aria-describedby={description ? `${uid}-desc` : undefined}
    onchange={(e) => onchange(e.currentTarget.checked)}
  />
</div>

<style>
  .switch {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
  }

  p {
    font-size: 0.85rem;
  }

  input {
    appearance: none;
    flex: none;
    position: relative;
    width: 2.6rem;
    height: 1.5rem;
    margin: 0;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: var(--surface-2);
    cursor: pointer;
    transition: background 0.15s;
  }

  input::before {
    content: '';
    position: absolute;
    top: 2px;
    left: 2px;
    width: calc(1.5rem - 6px);
    height: calc(1.5rem - 6px);
    border-radius: 50%;
    background: var(--text-muted);
    transition: transform 0.15s;
  }

  input:checked {
    background: var(--accent);
    border-color: var(--accent);
  }

  input:checked::before {
    background: var(--accent-text);
    transform: translateX(1.1rem);
  }

  input:disabled {
    opacity: 0.5;
    cursor: default;
  }
</style>
