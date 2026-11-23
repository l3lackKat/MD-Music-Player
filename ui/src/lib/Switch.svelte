<script lang="ts">
  // An on/off switch. `label` is for screen readers; show the words next to it.
  let {
    checked,
    label,
    disabled = false,
    tone = "accent",
    onchange,
  }: {
    checked: boolean;
    label: string;
    disabled?: boolean;
    /** `warn` for a switch whose "on" gives something up, such as bit-perfect playback. */
    tone?: "accent" | "warn";
    onchange: () => void;
  } = $props();
</script>

<button
  role="switch"
  aria-checked={checked}
  aria-label={label}
  {disabled}
  class="relative h-6 w-11 shrink-0 rounded-full transition-colors duration-200 disabled:cursor-not-allowed disabled:opacity-40 {checked
    ? tone === 'warn'
      ? 'bg-warn'
      : 'bg-accent'
    : 'bg-line'}"
  onclick={onchange}
>
  <span
    class="absolute top-1 h-4 w-4 rounded-full bg-ink shadow transition-[left] duration-200 {checked ? 'left-6' : 'left-1'}"
  ></span>
</button>
