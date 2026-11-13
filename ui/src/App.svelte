<script lang="ts">
  import { onMount } from "svelte";
  import { confirm, open } from "@tauri-apps/plugin-dialog";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import {
    api,
    listenPlayer,
    formatRate,
    formatTime,
    type DeviceInfo,
    type OutputMode,
    type Playlist,
    type PlayState,
    type SignalPath,
    type TrackMeta,
  } from "./lib/api";
  import Cover from "./lib/Cover.svelte";
  import NowPlaying from "./lib/NowPlaying.svelte";
  import TrackTable from "./lib/TrackTable.svelte";

  // ---- Player ----
  let queue = $state<TrackMeta[]>([]);
  let currentIndex = $state<number | null>(null);
  let playState = $state<PlayState>("stopped");
  let positionMs = $state(0);
  let durationMs = $state<number | null>(null);
  let signalPath = $state<SignalPath | null>(null);

  // ---- Output ----
  let devices = $state<DeviceInfo[]>([]);
  let deviceId = $state<string | null>(null);
  let mode = $state<OutputMode>("exclusive");

  // ---- Playlists ----
  let playlists = $state<Playlist[]>([]);
  /** What the main area shows: the queue, or the playlist with this id. */
  let view = $state<"queue" | number>("queue");
  /** Tracks found by the last scan of each playlist, by playlist id. */
  let scanned = $state<Record<number, TrackMeta[]>>({});
  let scanningId = $state<number | null>(null);
  let newName = $state<string | null>(null);
  let renaming = $state<string | null>(null);

  // ---- Window ----
  let error = $state<string | null>(null);
  let dragging = $state(false);
  let showPanel = $state(true);
  // While the thumb is held, the bar follows the pointer, not the player.
  let scrubMs = $state<number | null>(null);

  const shownMs = $derived(scrubMs ?? positionMs);
  const fill = $derived(durationMs ? Math.min(100, (shownMs / durationMs) * 100) : 0);
  const currentTrack = $derived(currentIndex !== null ? (queue[currentIndex] ?? null) : null);
  const playlist = $derived(view === "queue" ? null : (playlists.find((p) => p.id === view) ?? null));
  const playlistTracks = $derived(playlist ? (scanned[playlist.id] ?? null) : null);

  const sourceLabel = $derived.by(() => {
    if (!signalPath) return "";
    const s = signalPath.source;
    if (s.kind.type === "dsd") return `${s.codec} · ${(s.sample_rate / 1e6).toFixed(1)} MHz`;
    const bits = s.kind.type === "pcm" ? `${s.kind.bits}-bit · ` : "";
    return `${s.codec} · ${bits}${formatRate(s.sample_rate)}`;
  });
  const outputLabel = $derived.by(() => {
    if (!signalPath) return "";
    const o = signalPath.output;
    return o.dop ? `DoP · ${formatRate(o.sample_rate)}` : `${o.valid_bits}-bit · ${formatRate(o.sample_rate)}`;
  });

  function showError(message: unknown) {
    error = String(message);
  }

  // ---- Queue and transport ----

  async function playQueue(index: number) {
    error = null;
    await api.playFiles(queue.map((t) => t.path), index).catch(showError);
  }

  /** Replace the queue with these tracks and play from `index`. */
  async function playTracks(tracks: TrackMeta[], index: number) {
    queue = [...tracks];
    await playQueue(index);
  }

  /** Add files and folders to the end of the queue; start playing if idle. */
  async function addToQueue(paths: string[]) {
    const tracks = await api.expandPaths(paths).catch((e) => (showError(e), []));
    if (tracks.length === 0) return;
    const startAt = queue.length;
    const wasIdle = playState === "stopped";
    queue = [...queue, ...tracks];
    if (wasIdle) await playQueue(startAt);
  }

  async function pickFiles() {
    const extensions = await api.supportedExtensions();
    const picked = await open({ multiple: true, filters: [{ name: "Audio", extensions }] });
    if (picked) await addToQueue(picked);
  }

  async function togglePlay() {
    error = null;
    if (playState === "playing") await api.pause().catch(showError);
    else if (playState === "paused") await api.play().catch(showError);
    else if (queue.length > 0) await playQueue(currentIndex ?? 0);
  }

  async function clearQueue() {
    await api.stop().catch(showError);
    queue = [];
    currentIndex = null;
    signalPath = null;
    positionMs = 0;
    durationMs = null;
  }

  async function seekTo(ms: number) {
    positionMs = ms;
    await api.seek(ms).catch(showError);
  }

  async function commitSeek() {
    if (scrubMs === null) return;
    const target = scrubMs;
    scrubMs = null;
    await seekTo(target);
  }

  // ---- Output ----

  async function applyOutput() {
    error = null;
    await api.setOutput(deviceId, mode).catch(showError);
  }

  async function refreshDevices() {
    devices = await api.listDevices().catch((e) => (showError(e), []));
  }

  // ---- Playlists ----

  function replacePlaylist(updated: Playlist) {
    playlists = playlists.map((p) => (p.id === updated.id ? updated : p));
  }

  async function scan(id: number) {
    scanningId = id;
    try {
      scanned[id] = await api.playlistScan(id);
    } catch (e) {
      showError(e);
    } finally {
      if (scanningId === id) scanningId = null;
    }
  }

  function openPlaylist(id: number) {
    view = id;
    renaming = null;
    if (!(id in scanned)) void scan(id);
  }

  async function createPlaylist() {
    const name = newName?.trim();
    newName = null;
    if (!name) return;
    try {
      const created = await api.playlistCreate(name);
      playlists = [...playlists, created];
      scanned[created.id] = [];
      view = created.id;
    } catch (e) {
      showError(e);
    }
  }

  async function renamePlaylist() {
    const name = renaming?.trim();
    renaming = null;
    if (!playlist || !name || name === playlist.name) return;
    await api.playlistRename(playlist.id, name).then(replacePlaylist).catch(showError);
  }

  async function deletePlaylist() {
    if (!playlist) return;
    const yes = await confirm(`Delete the playlist "${playlist.name}"? Your music files are not deleted.`, {
      title: "Delete playlist",
      kind: "warning",
    });
    if (!yes) return;
    const id = playlist.id;
    try {
      await api.playlistDelete(id);
      playlists = playlists.filter((p) => p.id !== id);
      delete scanned[id];
      view = "queue";
    } catch (e) {
      showError(e);
    }
  }

  /** Add folders to a playlist, then rescan it. Returns how many were added. */
  async function addFolders(id: number, folders: string[]) {
    let added = 0;
    let failure: unknown = null;
    for (const folder of folders) {
      try {
        replacePlaylist(await api.playlistAddFolder(id, folder));
        added++;
      } catch (e) {
        failure = e;
      }
    }
    if (failure !== null) showError(failure);
    if (added > 0) await scan(id);
  }

  async function pickFolders() {
    if (!playlist) return;
    const picked = await open({ directory: true, multiple: true, title: "Add folders to the playlist" });
    if (picked) await addFolders(playlist.id, picked);
  }

  async function removeFolder(folder: string) {
    if (!playlist) return;
    const id = playlist.id;
    await api.playlistRemoveFolder(id, folder).then(replacePlaylist).catch(showError);
    await scan(id);
  }

  // ---- Startup ----

  onMount(() => {
    const cleanups: (() => void)[] = [];
    (async () => {
      const output = await api.getOutput();
      deviceId = output.device_id;
      mode = output.mode;
      playlists = await api.playlists();
      await refreshDevices();

      cleanups.push(
        await listenPlayer({
          state: (s) => (playState = s),
          track: (t) => {
            currentIndex = t.index;
            signalPath = t.signal_path;
          },
          position: (p) => {
            positionMs = p.ms;
            if (p.duration_ms !== null || playState !== "stopped") durationMs = p.duration_ms;
          },
          error: showError,
          queueEnded: () => (positionMs = 0),
        }),
      );
      cleanups.push(
        await getCurrentWebview().onDragDropEvent((event) => {
          if (event.payload.type === "over" || event.payload.type === "enter") dragging = true;
          else if (event.payload.type === "leave") dragging = false;
          else if (event.payload.type === "drop") {
            dragging = false;
            // On a playlist, a drop adds folders to it; on the queue, it adds tracks.
            if (view === "queue") void addToQueue(event.payload.paths);
            else void addFolders(view, event.payload.paths);
          }
        }),
      );
    })().catch(showError);
    return () => cleanups.forEach((c) => c());
  });

  function onKey(event: KeyboardEvent) {
    const tag = (event.target as HTMLElement).tagName;
    if (event.code === "Space" && !["SELECT", "BUTTON", "INPUT"].includes(tag)) {
      event.preventDefault();
      void togglePlay();
    }
  }

  /** Focus and select an input as soon as it appears. */
  function focusOnMount(node: HTMLInputElement) {
    node.focus();
    node.select();
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="flex h-full flex-col">
  <!-- Top bar: output device and mode -->
  <header class="flex items-center gap-3 border-b border-line bg-panel px-5 py-3">
    <div class="mr-auto flex items-center gap-2.5">
      <span class="grid h-7 w-7 place-items-center rounded-lg bg-accent/15 text-accent">
        <svg viewBox="0 0 24 24" class="h-4 w-4" fill="none" stroke="currentColor" stroke-width="2">
          <circle cx="12" cy="12" r="9" /><circle cx="12" cy="12" r="2.5" fill="currentColor" />
        </svg>
      </span>
      <span class="text-[15px] font-semibold tracking-tight">MDMusic</span>
    </div>

    <label class="text-xs text-muted" for="device">Output</label>
    <select
      id="device"
      class="max-w-72 truncate rounded-lg border border-line bg-raised px-3 py-1.5 text-sm outline-none focus:border-accent"
      bind:value={deviceId}
      onchange={applyOutput}
      onfocus={refreshDevices}
    >
      <option value={null}>System default</option>
      {#each devices as device (device.id)}
        <option value={device.id}>{device.name}</option>
      {/each}
    </select>

    <div class="flex rounded-lg border border-line bg-raised p-0.5 text-sm" role="group" aria-label="Output mode">
      {#each ["exclusive", "shared"] as const as option}
        <button
          class="rounded-md px-3 py-1 capitalize transition-colors {mode === option
            ? 'bg-accent/20 text-accent'
            : 'text-muted hover:text-ink'}"
          title={option === "exclusive"
            ? "Bit-perfect: MDMusic takes over the device, other apps are silent"
            : "Goes through the Windows mixer, other apps keep playing"}
          onclick={() => {
            mode = option;
            void applyOutput();
          }}
        >
          {option}
        </button>
      {/each}
    </div>
  </header>

  {#if error}
    <div class="flex items-start gap-3 border-b border-bad/30 bg-bad/10 px-5 py-2.5 text-sm text-bad">
      <span class="flex-1 select-text">{error}</span>
      <button class="text-bad/70 hover:text-bad" onclick={() => (error = null)} aria-label="Dismiss">✕</button>
    </div>
  {/if}

  <div class="flex min-h-0 flex-1">
    <!-- Sidebar: queue and playlists -->
    <nav class="flex w-52 shrink-0 flex-col gap-0.5 overflow-y-auto border-r border-line bg-panel p-3 text-sm">
      <button
        class="flex items-center justify-between rounded-lg px-3 py-2 text-left {view === 'queue'
          ? 'bg-raised font-medium text-ink'
          : 'text-muted hover:bg-raised hover:text-ink'}"
        onclick={() => (view = "queue")}
      >
        <span>Queue</span>
        {#if queue.length > 0}<span class="text-xs tabular-nums text-muted">{queue.length}</span>{/if}
      </button>

      <p class="px-3 pb-1 pt-4 text-xs font-medium uppercase tracking-wider text-muted">Playlists</p>
      {#each playlists as item (item.id)}
        <button
          class="truncate rounded-lg px-3 py-2 text-left {view === item.id
            ? 'bg-raised font-medium text-ink'
            : 'text-muted hover:bg-raised hover:text-ink'}"
          onclick={() => openPlaylist(item.id)}
          title={item.name}
        >
          {item.name}
        </button>
      {/each}

      {#if newName !== null}
        <input
          class="rounded-lg border border-accent bg-raised px-3 py-1.5 outline-none"
          placeholder="Playlist name"
          bind:value={newName}
          use:focusOnMount
          onblur={createPlaylist}
          onkeydown={(e) => {
            if (e.key === "Enter") void createPlaylist();
            else if (e.key === "Escape") newName = null;
          }}
        />
      {:else}
        <button
          class="rounded-lg px-3 py-2 text-left text-muted hover:bg-raised hover:text-ink"
          onclick={() => (newName = "")}
        >
          + New playlist
        </button>
      {/if}
    </nav>

    <main class="relative min-w-0 flex-1 overflow-y-auto">
      {#if playlist}
        <!-- A playlist: its folders and the tracks found in them -->
        <div class="px-6 pb-3 pt-5">
          <div class="flex items-center gap-3">
            {#if renaming !== null}
              <input
                class="min-w-0 flex-1 rounded-lg border border-accent bg-raised px-3 py-1 text-xl font-semibold outline-none"
                bind:value={renaming}
                use:focusOnMount
                onblur={renamePlaylist}
                onkeydown={(e) => {
                  if (e.key === "Enter") void renamePlaylist();
                  else if (e.key === "Escape") renaming = null;
                }}
              />
            {:else}
              <h1 class="min-w-0 flex-1 truncate text-xl font-semibold">{playlist.name}</h1>
            {/if}
            <div class="flex shrink-0 gap-1 text-sm">
              <button
                class="rounded-lg bg-accent px-3.5 py-1.5 font-medium text-base hover:brightness-110 disabled:opacity-40"
                disabled={!playlistTracks || playlistTracks.length === 0}
                onclick={() => playlistTracks && playTracks(playlistTracks, 0)}
              >
                Play
              </button>
              <button class="rounded-lg px-2.5 py-1.5 text-muted hover:bg-raised hover:text-ink" onclick={pickFolders}>
                Add folder
              </button>
              <button
                class="rounded-lg px-2.5 py-1.5 text-muted hover:bg-raised hover:text-ink disabled:opacity-40"
                disabled={scanningId === playlist.id}
                title="Look in the folders again for new or removed files"
                onclick={() => playlist && scan(playlist.id)}
              >
                Rescan
              </button>
              <button
                class="rounded-lg px-2.5 py-1.5 text-muted hover:bg-raised hover:text-ink"
                onclick={() => (renaming = playlist?.name ?? "")}
              >
                Rename
              </button>
              <button class="rounded-lg px-2.5 py-1.5 text-muted hover:bg-bad/15 hover:text-bad" onclick={deletePlaylist}>
                Delete
              </button>
            </div>
          </div>

          <p class="mt-1 text-xs text-muted">
            {#if scanningId === playlist.id}
              Scanning folders…
            {:else if playlistTracks}
              {playlistTracks.length} {playlistTracks.length === 1 ? "track" : "tracks"}
              · {formatTime(playlistTracks.reduce((sum, t) => sum + (t.duration_ms ?? 0), 0))}
            {/if}
          </p>

          {#if playlist.folders.length > 0}
            <ul class="mt-3 flex flex-wrap gap-2">
              {#each playlist.folders as folder (folder)}
                <li class="flex max-w-full items-center gap-1.5 rounded-full border border-line bg-raised py-1 pl-3 pr-1.5 text-xs">
                  <span class="truncate" title={folder}>{folder}</span>
                  <button
                    class="grid h-4 w-4 shrink-0 place-items-center rounded-full text-muted hover:bg-line hover:text-ink"
                    title="Remove this folder from the playlist"
                    aria-label="Remove folder {folder}"
                    onclick={() => removeFolder(folder)}
                  >
                    ✕
                  </button>
                </li>
              {/each}
            </ul>
          {/if}
        </div>

        {#if playlist.folders.length === 0}
          <div class="grid place-items-center px-8 py-16 text-center">
            <div class="max-w-sm">
              <p class="text-lg font-medium">This playlist has no folders</p>
              <p class="mt-1.5 text-sm text-muted">
                Add a folder and every audio file inside it, including subfolders, becomes part of the playlist.
              </p>
              <button
                class="mt-5 rounded-lg bg-accent px-4 py-2 text-sm font-medium text-base hover:brightness-110"
                onclick={pickFolders}
              >
                Add folder
              </button>
            </div>
          </div>
        {:else if playlistTracks && playlistTracks.length > 0}
          <TrackTable
            tracks={playlistTracks}
            currentPath={currentTrack?.path ?? null}
            playing={playState === "playing"}
            onplay={(index) => playTracks(playlistTracks, index)}
          />
        {:else if playlistTracks && scanningId !== playlist.id}
          <p class="px-6 py-10 text-center text-sm text-muted">No audio files were found in these folders.</p>
        {/if}
      {:else if queue.length === 0}
        <div class="grid h-full place-items-center p-8">
          <div class="max-w-sm text-center">
            <p class="text-lg font-medium">Nothing in the queue</p>
            <p class="mt-1.5 text-sm text-muted">
              Drop audio files or folders here, or pick files. FLAC, WAV, AIFF, ALAC, MP3, AAC, Ogg, DSF and DFF are
              supported.
            </p>
            <button
              class="mt-5 rounded-lg bg-accent px-4 py-2 text-sm font-medium text-base hover:brightness-110"
              onclick={pickFiles}
            >
              Add files
            </button>
          </div>
        </div>
      {:else}
        <div class="flex items-center justify-between px-6 pb-3 pt-5">
          <h1 class="text-xl font-semibold">Queue</h1>
          <div class="flex gap-1 text-sm">
            <button class="rounded-lg px-2.5 py-1.5 text-muted hover:bg-raised hover:text-ink" onclick={pickFiles}>
              Add files
            </button>
            <button class="rounded-lg px-2.5 py-1.5 text-muted hover:bg-raised hover:text-ink" onclick={clearQueue}>
              Clear
            </button>
          </div>
        </div>
        <TrackTable
          tracks={queue}
          currentPath={currentTrack?.path ?? null}
          playing={playState === "playing"}
          onplay={playQueue}
        />
      {/if}

      {#if dragging}
        <div class="pointer-events-none absolute inset-3 grid place-items-center rounded-2xl border-2 border-dashed border-accent bg-accent/10">
          <p class="font-medium text-accent">
            {playlist ? `Drop folders to add them to "${playlist.name}"` : "Drop to add to the queue"}
          </p>
        </div>
      {/if}
    </main>

    {#if showPanel}
      <NowPlaying track={currentTrack} {positionMs} onseek={seekTo} />
    {/if}
  </div>

  <!-- Transport -->
  <footer class="border-t border-line bg-panel px-5 pb-4 pt-3">
    <div class="flex items-center gap-3">
      <span class="w-12 text-right text-xs tabular-nums text-muted">{formatTime(shownMs)}</span>
      <input
        class="seek flex-1"
        type="range"
        min="0"
        max={durationMs ?? 0}
        step="100"
        value={shownMs}
        style="--fill: {fill}%"
        disabled={!durationMs || playState === "stopped"}
        aria-label="Seek"
        oninput={(e) => (scrubMs = Number(e.currentTarget.value))}
        onchange={commitSeek}
      />
      <span class="w-12 text-xs tabular-nums text-muted">{durationMs ? formatTime(durationMs) : "–:––"}</span>
    </div>

    <div class="mt-3 grid grid-cols-[1fr_auto_1fr] items-center gap-4">
      <div class="flex min-w-0 items-center gap-3">
        <Cover path={currentTrack?.path ?? null} class="h-11 w-11 shrink-0 rounded-md" />
        <div class="min-w-0">
          <p class="truncate text-sm font-medium">{currentTrack?.title ?? "Not playing"}</p>
          {#if currentTrack?.artist}
            <p class="mt-0.5 truncate text-xs text-muted">{currentTrack.artist}</p>
          {/if}
        </div>
      </div>

      <div class="flex items-center gap-2">
        <button
          class="grid h-9 w-9 place-items-center rounded-full text-muted hover:bg-raised hover:text-ink disabled:opacity-40"
          onclick={() => api.previous().catch(showError)}
          disabled={queue.length === 0}
          aria-label="Previous"
        >
          <svg viewBox="0 0 24 24" class="h-5 w-5" fill="currentColor"><path d="M6 5h2v14H6zM20 5v14L9 12z" /></svg>
        </button>
        <button
          class="grid h-11 w-11 place-items-center rounded-full bg-ink text-base hover:scale-105 disabled:opacity-40"
          onclick={togglePlay}
          disabled={queue.length === 0}
          aria-label={playState === "playing" ? "Pause" : "Play"}
        >
          {#if playState === "playing"}
            <svg viewBox="0 0 24 24" class="h-5 w-5" fill="currentColor"><path d="M7 5h4v14H7zM13 5h4v14h-4z" /></svg>
          {:else}
            <svg viewBox="0 0 24 24" class="h-5 w-5" fill="currentColor"><path d="M8 5v14l11-7z" /></svg>
          {/if}
        </button>
        <button
          class="grid h-9 w-9 place-items-center rounded-full text-muted hover:bg-raised hover:text-ink disabled:opacity-40"
          onclick={() => api.next().catch(showError)}
          disabled={queue.length === 0}
          aria-label="Next"
        >
          <svg viewBox="0 0 24 24" class="h-5 w-5" fill="currentColor"><path d="M16 5h2v14h-2zM4 5v14l11-7z" /></svg>
        </button>
      </div>

      <!-- Signal path: what the file is, what the device receives -->
      <div class="flex min-w-0 items-center justify-end gap-2 text-xs">
        {#if signalPath && playState !== "stopped"}
          <span class="truncate text-muted" title={signalPath.device}>{sourceLabel} → {outputLabel}</span>
          <span
            class="shrink-0 rounded-full px-2.5 py-1 font-medium {signalPath.bit_perfect
              ? 'bg-good/15 text-good'
              : 'bg-warn/15 text-warn'}"
            title={signalPath.bit_perfect
              ? "Every sample reaches the DAC unchanged"
              : signalPath.exclusive
                ? "The source is lossy or floating point, so there are no original samples to preserve"
                : "Shared mode: Windows resamples and mixes the audio"}
          >
            {signalPath.bit_perfect ? "Bit-perfect" : signalPath.exclusive ? "Exclusive" : "Shared"}
          </span>
        {/if}
        <button
          class="grid h-8 w-8 shrink-0 place-items-center rounded-lg hover:bg-raised {showPanel
            ? 'text-accent'
            : 'text-muted hover:text-ink'}"
          title={showPanel ? "Hide cover and lyrics" : "Show cover and lyrics"}
          aria-pressed={showPanel}
          onclick={() => (showPanel = !showPanel)}
        >
          <svg viewBox="0 0 24 24" class="h-4.5 w-4.5" fill="none" stroke="currentColor" stroke-width="2">
            <rect x="3" y="4" width="18" height="16" rx="2" /><path d="M15 4v16" />
          </svg>
        </button>
      </div>
    </div>
  </footer>
</div>
