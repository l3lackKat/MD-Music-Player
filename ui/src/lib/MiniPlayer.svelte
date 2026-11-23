<script lang="ts">
  // The mini player: a small window that stays on top, with the cover, the
  // track, a seek bar and the transport. It drives the same player as the
  // main window, which is hidden meanwhile and keeps the queue; the expand
  // button brings it back. Drag the window by any empty part.
  import { onMount } from "svelte";
  import {
    api,
    coverUrl,
    fileName,
    formatTime,
    listenOutput,
    listenPlayer,
    type PlayState,
    type PositionEvent,
    type TrackMeta,
  } from "./api";

  /** How long an error or note stays in place of the artist. */
  const MESSAGE_MS = 6000;

  let playState = $state<PlayState>("stopped");
  let track = $state<TrackMeta | null>(null);
  let positionMs = $state(0);
  let durationMs = $state<number | null>(null);
  /** The path whose cover failed to load, so the next track gets a fresh try. */
  let coverFailed = $state<string | null>(null);
  /** While the seek thumb is held, it shows where it is, not the player. */
  let scrubMs = $state<number | null>(null);
  /** The last error or note, shown in place of the artist for a while. */
  let message = $state<{ text: string; bad: boolean } | null>(null);
  let messageTimer: ReturnType<typeof setTimeout> | undefined;
  /** The unplugged device playback waits for, until it is back or the wait is called off. */
  let waitingFor = $state<string | null>(null);
  /** The path last asked for: a slower lookup for an older track is dropped. */
  let wantedPath: string | null = null;

  const shownMs = $derived(scrubMs ?? positionMs);
  const fill = $derived(durationMs ? Math.min(100, (shownMs / durationMs) * 100) : 0);

  async function showTrack(path: string | null) {
    wantedPath = path;
    if (path === null) {
      track = null;
      return;
    }
    if (track?.path === path) return;
    const found = await api.expandPaths([path]).catch(() => [] as TrackMeta[]);
    if (wantedPath !== path) return;
    track = found[0] ?? {
      path,
      title: fileName(path),
      artist: null,
      album: null,
      album_artist: null,
      track_no: null,
      disc_no: null,
      year: null,
      duration_ms: null,
    };
  }

  function takePosition(p: PositionEvent) {
    positionMs = p.ms;
    // Stopped, a position carries no duration: keep the track's. Playing, none means unknown.
    if (p.duration_ms !== null || playState !== "stopped") durationMs = p.duration_ms;
  }

  function say(text: string, bad = true) {
    message = { text, bad };
    clearTimeout(messageTimer);
    messageTimer = setTimeout(() => (message = null), MESSAGE_MS);
  }

  const fail = (e: unknown) => say(String(e));

  onMount(() => {
    const cleanups: (() => void)[] = [];
    let gone = false;
    // Events that come in before the snapshot are applied after it, in order, so the newest wins.
    let early: (() => void)[] | null = [];
    function on<A extends unknown[]>(apply: (...args: A) => void): (...args: A) => void {
      return (...args) => {
        if (early) early.push(() => apply(...args));
        else apply(...args);
      };
    }

    void (async () => {
      const offs = await Promise.all([
        listenPlayer({
          state: on((s) => (playState = s)),
          track: on((e) => void showTrack(e.path)),
          current: () => {},
          cued: on((cue) => {
            // Not opened yet, so its length is unknown.
            if (cue.path !== wantedPath) durationMs = null;
            void showTrack(cue.path);
          }),
          position: on(takePosition),
          error: on((m) => say(m)),
          deviceLost: on((m) => say(m)),
          queueEnded: () => {},
        }),
        listenOutput({
          waiting: on((device) => (waitingFor = device)),
          reconnected: on(({ device }) => {
            waitingFor = null;
            say(`${device} is back`, false);
          }),
          waitEnded: on(() => (waitingFor = null)),
        }),
      ]).catch((e) => (fail(e), [] as (() => void)[]));
      cleanups.push(...offs);
      if (gone) {
        cleanups.forEach((off) => off());
        return;
      }
      const now = await api.nowPlaying().catch(() => null);
      if (now) {
        playState = now.state;
        positionMs = now.position_ms;
        durationMs = now.duration_ms;
        void showTrack(now.path);
      }
      const pending = early ?? [];
      early = null;
      pending.forEach((apply) => apply());
    })();

    return () => {
      gone = true;
      cleanups.forEach((off) => off());
      clearTimeout(messageTimer);
    };
  });

  function togglePlay() {
    void (playState === "playing" ? api.pause() : api.play()).catch(fail);
  }

  function seekTo(ms: number) {
    void api
      .seek(ms)
      .catch(fail)
      // Show the new place at once, not the old one until the next position event.
      .finally(() => {
        positionMs = ms;
        scrubMs = null;
      });
  }

  function onKey(e: KeyboardEvent) {
    if (e.code === "Space" && !(e.target instanceof HTMLInputElement)) {
      e.preventDefault();
      togglePlay();
    }
  }

  const ICON = "grid h-8 w-8 place-items-center rounded-full text-muted hover:bg-raised hover:text-ink disabled:opacity-40";
</script>

<svelte:window onkeydown={onKey} />

<!-- `data-tauri-drag-region` marks what drags the window; it does not reach children, so the text has it too. -->
<div class="relative flex h-screen items-center gap-3 border border-line bg-panel px-3" data-tauri-drag-region>
  <span class="grid h-[76px] w-[76px] shrink-0 place-items-center overflow-hidden rounded-md bg-raised text-muted/60">
    {#if track && coverFailed !== track.path}
      <!-- A fresh element per track, so a late error from the old cover cannot hide the new one. -->
      {#key track.path}
        <img
          src={coverUrl(track.path)}
          alt=""
          draggable="false"
          class="pointer-events-none h-full w-full object-cover"
          onerror={() => (coverFailed = track?.path ?? null)}
        />
      {/key}
    {:else}
      <svg viewBox="0 0 24 24" class="h-1/3 w-1/3" fill="currentColor" aria-hidden="true">
        <path d="M10 17.5a2.5 2.5 0 1 1-2-2.45V5l10-2v11.5a2.5 2.5 0 1 1-2-2.45V7.1l-6 1.2z" />
      </svg>
    {/if}
  </span>

  <div class="min-w-0 flex-1" data-tauri-drag-region>
    <p class="truncate pr-7 text-sm font-semibold" title={track?.title} data-tauri-drag-region>
      {track?.title ?? "Not playing"}
    </p>
    {#if waitingFor !== null}
      <p class="mt-0.5 min-h-4 truncate pr-7 text-xs text-warn" title="Playback resumes when it is plugged back in" data-tauri-drag-region>
        Waiting for {waitingFor}
      </p>
    {:else if message}
      <p class="mt-0.5 min-h-4 truncate pr-7 text-xs {message.bad ? 'text-bad' : 'text-good'}" title={message.text} role="status" data-tauri-drag-region>
        {message.text}
      </p>
    {:else}
      <p class="mt-0.5 min-h-4 truncate pr-7 text-xs text-muted" data-tauri-drag-region>{track?.artist ?? ""}</p>
    {/if}

    <div class="mt-1 flex items-center gap-0.5" data-tauri-drag-region>
      <button class={ICON} aria-label="Previous" title="Previous" disabled={!track} onclick={() => api.previous().catch(fail)}>
        <svg viewBox="0 0 24 24" class="h-4 w-4" fill="currentColor"><path d="M6 5h2v14H6zM20 5v14L9 12z" /></svg>
      </button>
      <button
        class="grid h-8 w-8 place-items-center rounded-full bg-ink text-base hover:scale-105 disabled:opacity-40"
        aria-label={playState === "playing" ? "Pause" : "Play"}
        disabled={!track}
        onclick={togglePlay}
      >
        {#if playState === "playing"}
          <svg viewBox="0 0 24 24" class="h-4 w-4" fill="currentColor"><path d="M7 5h4v14H7zM13 5h4v14h-4z" /></svg>
        {:else}
          <svg viewBox="0 0 24 24" class="h-4 w-4" fill="currentColor"><path d="M8 5v14l11-7z" /></svg>
        {/if}
      </button>
      <button class={ICON} aria-label="Next" title="Next" disabled={!track} onclick={() => api.next().catch(fail)}>
        <svg viewBox="0 0 24 24" class="h-4 w-4" fill="currentColor"><path d="M16 5h2v14h-2zM4 5v14l11-7z" /></svg>
      </button>
      <span class="ml-auto text-[11px] tabular-nums text-muted" data-tauri-drag-region>
        {formatTime(shownMs)}{durationMs ? ` / ${formatTime(durationMs)}` : ""}
      </span>
    </div>

    <input
      class="seek mt-1.5 block w-full"
      type="range"
      min="0"
      max={durationMs ?? 0}
      step="any"
      value={shownMs}
      style="--fill: {fill / 100}"
      disabled={!durationMs || playState === "stopped"}
      aria-label="Seek"
      oninput={(e) => (scrubMs = Math.round(Number(e.currentTarget.value)))}
      onchange={(e) => seekTo(Math.round(Number(e.currentTarget.value)))}
    />
  </div>

  <button
    class="absolute right-2 top-2 grid h-7 w-7 place-items-center rounded-md text-muted hover:bg-raised hover:text-ink"
    title="Back to the full player"
    aria-label="Back to the full player"
    onclick={() => api.closeMiniPlayer().catch(fail)}
  >
    <svg viewBox="0 0 24 24" class="h-4 w-4" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <path d="M15 3h6v6M9 21H3v-6M21 3l-7 7M3 21l7-7" />
    </svg>
  </button>
</div>
