<script lang="ts">
  // A track's cover art, with a placeholder when the track has none.
  // With `onclick` it is a button; `disc` shows it as a round record that
  // turns while `spinning`.
  import { coverUrl } from "./api";

  let {
    path,
    class: className = "",
    disc = false,
    spinning = false,
    onclick,
    title,
  }: {
    path: string | null;
    class?: string;
    /** Round, with a spindle hole, like a record. */
    disc?: boolean;
    /** The record is turning. When false it stops where it is, like a paused turntable. */
    spinning?: boolean;
    onclick?: () => void;
    title?: string;
  } = $props();

  // The path whose image failed to load, so a new track gets a fresh try.
  let failedPath = $state<string | null>(null);
  const show = $derived(path !== null && failedPath !== path);
</script>

<!-- The frame keeps the shadow and the round edge still; only the picture inside turns. -->
{#snippet inside()}
  <span class="picture absolute inset-0 grid place-items-center" class:turning={disc} class:paused={!spinning}>
    {#if show && path}
      <img
        src={coverUrl(path)}
        alt=""
        class="h-full w-full object-cover"
        draggable="false"
        onerror={() => (failedPath = path)}
      />
    {:else}
      <svg viewBox="0 0 24 24" class="h-2/5 w-2/5" fill="none" stroke="currentColor" stroke-width="1.5">
        <circle cx="12" cy="12" r="9" /><circle cx="12" cy="12" r="2.5" />
      </svg>
    {/if}
  </span>
  {#if disc}
    <span class="hole" aria-hidden="true"></span>
  {/if}
{/snippet}

{#if onclick}
  <button
    type="button"
    class="frame relative block cursor-pointer overflow-hidden bg-raised text-line {className}"
    class:disc
    {title}
    aria-pressed={disc}
    {onclick}
  >
    {@render inside()}
  </button>
{:else}
  <div class="frame relative block overflow-hidden bg-raised text-line {className}" class:disc {title}>
    {@render inside()}
  </div>
{/if}

<style>
  .frame {
    transition: border-radius 0.35s ease;
  }
  /* Beats the corner radius the caller passes in `class`. */
  .frame.disc {
    border-radius: 50%;
  }

  .picture.turning {
    animation: turn 20s linear infinite;
  }
  .picture.paused {
    animation-play-state: paused;
  }
  @keyframes turn {
    to {
      transform: rotate(360deg);
    }
  }

  /* The spindle hole, with a darker ring for the label's edge. */
  .hole {
    position: absolute;
    inset: 0;
    margin: auto;
    width: 13%;
    height: 13%;
    border-radius: 50%;
    background: var(--color-panel);
    box-shadow:
      0 0 0 1px rgb(0 0 0 / 0.45),
      0 0 0 4px rgb(0 0 0 / 0.18);
  }

  @media (prefers-reduced-motion: reduce) {
    .picture.turning {
      animation: none;
    }
  }
</style>
