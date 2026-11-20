<script lang="ts">
  import { onDestroy, onMount, tick, untrack } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import { confirm, open } from "@tauri-apps/plugin-dialog";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import {
    api,
    holdsFile,
    listenLibrary,
    listenOutput,
    listenPlayer,
    formatKhz,
    formatRate,
    formatTime,
    type DeviceInfo,
    type EqSettings,
    type OutputMode,
    type OutputSettings,
    type Playlist,
    type PlayState,
    type Repeat,
    type SignalPath,
    type TrackMeta,
  } from "./lib/api";
  import ContextMenu, { type MenuItem } from "./lib/ContextMenu.svelte";
  import Cover from "./lib/Cover.svelte";
  import EqPanel from "./lib/EqPanel.svelte";
  import NowPlaying from "./lib/NowPlaying.svelte";
  import { isIdentity, moveRows, RowDrag } from "./lib/reorder.svelte";
  import { loadSorts, nextSort, saveSorts, sortTracks, type PlaylistSorts, type SortKey } from "./lib/sort";
  import SourceMark from "./lib/SourceMark.svelte";
  import Spinner from "./lib/Spinner.svelte";
  import TrackInfo from "./lib/TrackInfo.svelte";
  import TrackTable from "./lib/TrackTable.svelte";
  import VolumeControl from "./lib/VolumeControl.svelte";
  import appIcon from "./assets/app-icon.png";

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
  /** The mode in use, which is shared while the device cannot do exclusive. */
  let mode = $state<OutputMode>("exclusive");
  /** The mode the user picked. */
  let wantedMode = $state<OutputMode>("exclusive");
  /** Exclusive needs a hardware volume, else the level is stuck at full. */
  let exclusiveAvailable = $state(true);
  /** The device has an adjustable volume of its own. */
  let hardwareVolume = $state(true);
  /** The software volume is on (it acts only on devices without a hardware volume, in exclusive mode). */
  let softVolumeEnabled = $state(false);
  let volumeControl = $state<ReturnType<typeof VolumeControl>>();
  /** The device that was unplugged mid-track; playback resumes when it is back. */
  let waitingFor = $state<string | null>(null);

  // ---- Playlists ----
  let playlists = $state<Playlist[]>([]);
  /** What the main area shows: the queue, search results, or the playlist with this id. */
  let view = $state<"queue" | "search" | number>("queue");
  /** Where to go back to when the search box is emptied. */
  let lastView: "queue" | number = "queue";

  // ---- Search ----
  let searchQuery = $state("");
  /** The last search's tracks; `null` before the first answer. */
  let searchResults = $state<TrackMeta[] | null>(null);
  let searching = $state(false);
  /** Tracks in the library index, which is what search looks through. */
  let libraryTracks = $state<number | null>(null);
  /** Tracks found by the last scan of each playlist, by playlist id. */
  let scanned = $state<Record<number, TrackMeta[]>>({});
  /** Ids of the playlists being scanned right now. */
  const scanning = new SvelteSet<number>();
  /** The newest scan started for each playlist. One that finishes after a newer one began is dropped. */
  const scanRun = new Map<number, number>();
  /** The column each playlist is sorted by; a playlist not listed is in its own order. */
  let sorts = $state<PlaylistSorts>(loadSorts());
  let newName = $state<string | null>(null);
  let renaming = $state<string | null>(null);

  let shuffle = $state(false);
  let repeat = $state<Repeat>("off");
  /** The equalizer, as last saved. `null` until loaded. */
  let eq = $state<EqSettings | null>(null);
  let showEq = $state(false);

  // ---- Window ----
  /** The open track menu: where and what. */
  let menu = $state<{ x: number; y: number; items: MenuItem[] } | null>(null);
  /** A short confirmation, such as "Added to Favourites". */
  let notice = $state<string | null>(null);
  let noticeTimer: ReturnType<typeof setTimeout> | undefined;
  let error = $state<string | null>(null);
  /** The track whose info dialog is open. */
  let infoTrack = $state<TrackMeta | null>(null);
  let dragging = $state(false);
  let showPanel = $state(true);
  /** Below this width the track list and the cover panel do not both fit (a 1080p
   *  screen split in half is 960 px), so the panel steps aside. */
  const NARROW_WIDTH = 1100;
  let innerWidth = $state(window.innerWidth);
  const narrow = $derived(innerWidth < NARROW_WIDTH);
  /** The panel was hidden by the window narrowing, not by the user: it comes back when the window widens. */
  let panelAutoHidden = false;
  $effect(() => {
    if (narrow) {
      if (untrack(() => showPanel)) {
        showPanel = false;
        panelAutoHidden = true;
      }
    } else if (panelAutoHidden) {
      showPanel = true;
      panelAutoHidden = false;
    }
  });
  /** The cover is shown as a round record that turns while playing. Clicking a cover switches it. */
  let disc = $state(loadDisc());
  // While the thumb is held, the bar follows the pointer, not the player.
  let scrubMs = $state<number | null>(null);

  // The player reports the position every 100 ms. Between reports the bar runs on
  // the clock from the last one, so it moves every frame instead of in steps.
  /** The last reported position and when it arrived (`performance.now()`). */
  let anchor = { ms: 0, at: 0 };
  let smoothMs = $state(0);
  /** How far past the last report the bar may run before it waits for the next one. */
  const AHEAD_LIMIT_MS = 250;
  /** A report this far behind the bar is jitter: the bar holds instead of stepping back. */
  const JITTER_MS = 400;

  function placePosition(ms: number, jump = false) {
    positionMs = ms;
    anchor = { ms, at: performance.now() };
    if (jump || playState !== "playing") smoothMs = ms;
  }

  $effect(() => {
    if (playState !== "playing") {
      smoothMs = positionMs;
      return;
    }
    anchor = { ms: untrack(() => positionMs), at: performance.now() };
    let frame = requestAnimationFrame(function step(now) {
      const next = anchor.ms + Math.min(now - anchor.at, AHEAD_LIMIT_MS);
      const end = durationMs ?? Infinity;
      const held = next < smoothMs && smoothMs - next < JITTER_MS;
      smoothMs = Math.min(held ? smoothMs : next, end);
      frame = requestAnimationFrame(step);
    });
    return () => cancelAnimationFrame(frame);
  });

  const shownMs = $derived(scrubMs ?? smoothMs);
  const fill = $derived(durationMs ? Math.min(100, (shownMs / durationMs) * 100) : 0);
  const currentTrack = $derived(currentIndex !== null ? (queue[currentIndex] ?? null) : null);
  const playlist = $derived(typeof view === "number" ? (playlists.find((p) => p.id === view) ?? null) : null);
  const playlistSort = $derived(playlist ? (sorts[playlist.id] ?? null) : null);
  /** The open playlist's tracks as listed and played: the last scan, in the chosen sort. */
  const playlistTracks = $derived.by(() => {
    const found = playlist ? (scanned[playlist.id] ?? null) : null;
    return found && sortTracks(found, playlistSort);
  });
  /** The open playlist is being scanned. */
  const playlistLoading = $derived(playlist !== null && scanning.has(playlist.id));

  /** The decoder's codec name, as a listener knows it: "PCM_S16LE" is plain PCM (WAV, AIFF). */
  function codecName(codec: string): string {
    if (codec.startsWith("PCM")) return "PCM";
    if (codec === "VORBIS") return "Vorbis";
    if (codec === "OPUS") return "Opus";
    return codec;
  }

  // The signal path in the transport bar, in the short form players use ("16/44.1 →
  // 24/44.1 kHz"), so it fits beside the volume; `signalTitle` spells it out, codec included.
  const sourceLabel = $derived.by(() => {
    if (!signalPath) return "";
    const s = signalPath.source;
    if (s.kind.type === "dsd") return s.codec;
    if (s.kind.type === "pcm") return `${s.kind.bits}/${formatKhz(s.sample_rate)}`;
    return `${codecName(s.codec)} ${formatRate(s.sample_rate)}`;
  });
  const outputLabel = $derived.by(() => {
    if (!signalPath) return "";
    const o = signalPath.output;
    return o.dop ? `DoP ${formatRate(o.sample_rate)}` : `${o.valid_bits}/${formatRate(o.sample_rate)}`;
  });
  const signalTitle = $derived.by(() => {
    if (!signalPath) return "";
    const { source: s, output: o } = signalPath;
    const file =
      s.kind.type === "dsd"
        ? `${s.codec} (${(s.sample_rate / 1e6).toFixed(1)} MHz)`
        : s.kind.type === "pcm"
          ? `${codecName(s.codec)}, ${s.kind.bits}-bit, ${formatRate(s.sample_rate)}`
          : `${codecName(s.codec)}, ${formatRate(s.sample_rate)}`;
    const words = o.container.slice(1);
    const sent = o.dop
      ? `DSD over PCM (DoP) at ${formatRate(o.sample_rate)}`
      : `${o.valid_bits}-bit${String(o.valid_bits) === words ? "" : ` in ${words}-bit words`}, ${formatRate(o.sample_rate)}`;
    return `File: ${file}\nThe DAC receives: ${sent}\nDevice: ${signalPath.device}`;
  });
  /** The bit-perfect badge's tooltip: why it says what it says, then the full path. */
  const badgeTitle = $derived.by(() => {
    if (!signalPath) return "";
    const why = signalPath.bit_perfect
      ? "Every sample reaches the DAC unchanged"
      : signalPath.eq
        ? "The equalizer is changing the samples. Turn it off for bit-perfect playback"
        : signalPath.soft_volume
          ? "The software volume is scaling the samples, since this device has no volume of its own. Use a device with a hardware volume for bit-perfect playback"
          : signalPath.exclusive
            ? "The source is lossy or floating point, so there are no original samples to preserve"
            : "Shared mode: Windows resamples and mixes the audio";
    return `${why}.\n\n${signalTitle}`;
  });

  function showError(message: unknown) {
    error = String(message);
  }

  function showNotice(message: string) {
    notice = message;
    clearTimeout(noticeTimer);
    noticeTimer = setTimeout(() => (notice = null), 2500);
  }

  function loadDisc(): boolean {
    try {
      return localStorage.getItem("coverDisc") === "1";
    } catch {
      return false;
    }
  }

  function toggleDisc() {
    disc = !disc;
    try {
      localStorage.setItem("coverDisc", disc ? "1" : "0");
    } catch {}
  }

  // ---- Queue and transport ----

  async function playQueue(index: number) {
    error = null;
    waitingFor = null;
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
    else await api.appendFiles(tracks.map((t) => t.path)).catch(showError);
    return tracks.length;
  }

  /** "Play next": put tracks right after the one playing; when idle, start playing them. */
  async function playNext(paths: string[]) {
    const tracks = await api.expandPaths(paths).catch((e) => (showError(e), []));
    if (tracks.length === 0) return;
    const at = currentIndex === null ? queue.length : currentIndex + 1;
    queue = [...queue.slice(0, at), ...tracks, ...queue.slice(at)];
    if (playState === "stopped") await playQueue(at);
    else await api.queueInsert(tracks.map((t) => t.path), at).catch(showError);
    return tracks.length;
  }

  async function toggleShuffle() {
    shuffle = !shuffle;
    try {
      localStorage.setItem("shuffle", shuffle ? "1" : "0");
    } catch {}
    await api.setShuffle(shuffle).catch(showError);
  }

  const REPEAT_NEXT: Record<Repeat, Repeat> = { off: "all", all: "one", one: "off" };
  const REPEAT_TITLE: Record<Repeat, string> = {
    off: "Repeat is off",
    all: "Repeat all: the queue starts over after the last track",
    one: "Repeat one: the current track plays again",
  };

  function loadRepeat(): Repeat {
    try {
      const saved = localStorage.getItem("repeat");
      if (saved === "all" || saved === "one") return saved;
    } catch {}
    return "off";
  }

  /** Off, then all, then one. */
  async function cycleRepeat() {
    repeat = REPEAT_NEXT[repeat];
    try {
      localStorage.setItem("repeat", repeat);
    } catch {}
    await api.setRepeat(repeat).catch(showError);
  }

  /** Take tracks out of the queue, here and in the engine. */
  async function removeFromQueue(rows: number[]) {
    const gone = new Set(rows.filter((i) => i >= 0 && i < queue.length));
    if (gone.size === 0) return;
    queue = queue.filter((_, i) => !gone.has(i));
    if (queue.length === 0) {
      // Nothing left: the engine stops on its own.
      currentIndex = null;
      signalPath = null;
      placePosition(0, true);
      durationMs = null;
    } else if (currentIndex !== null) {
      // The tracks before it that went. If it went itself, the engine moves on
      // and announces the next one; meanwhile point at the next that stayed.
      const c = currentIndex;
      currentIndex = Math.min([...Array(c).keys()].filter((i) => !gone.has(i)).length, queue.length - 1);
    }
    await api.queueRemove([...gone]).catch(showError);
  }

  /** Move queue rows into the gap before `slot`, here and in the engine. */
  async function moveInQueue(rows: number[], slot: number) {
    const order = moveRows(queue.length, rows, slot);
    if (isIdentity(order)) return;
    queue = order.map((old) => queue[old]);
    if (currentIndex !== null) currentIndex = order.indexOf(currentIndex);
    menu = null;
    await api.queueReorder(order).catch(showError);
  }

  async function pickFiles() {
    const extensions = await api.supportedExtensions();
    const picked = await open({ multiple: true, filters: [{ name: "Audio", extensions }] });
    if (picked) await addToQueue(picked);
  }

  async function togglePlay() {
    error = null;
    if (playState !== "playing") waitingFor = null;
    if (playState === "playing") await api.pause().catch(showError);
    else if (playState === "paused") await api.play().catch(showError);
    else if (queue.length > 0) await playQueue(currentIndex ?? 0);
  }

  async function clearQueue() {
    await api.stop().catch(showError);
    waitingFor = null;
    queue = [];
    currentIndex = null;
    signalPath = null;
    placePosition(0, true);
    durationMs = null;
  }

  async function seekTo(ms: number) {
    placePosition(ms, true);
    await api.seek(ms).catch(showError);
  }

  async function commitSeek() {
    if (scrubMs === null) return;
    const target = scrubMs;
    scrubMs = null;
    await seekTo(target);
  }

  // ---- Output ----

  function takeOutput(output: OutputSettings) {
    deviceId = output.device_id;
    mode = output.mode;
    wantedMode = output.wanted;
    exclusiveAvailable = output.exclusive_available;
    hardwareVolume = output.hardware_volume;
    softVolumeEnabled = output.soft_volume_enabled;
  }

  /** Turn the software volume on or off for devices without a hardware volume. */
  async function toggleSoftVolume() {
    error = null;
    waitingFor = null;
    try {
      const output = await api.setSoftVolume(!softVolumeEnabled);
      takeOutput(output);
      const volume = await volumeControl?.refresh();
      if (output.soft_volume)
        showNotice(
          `Software volume on at ${Math.round((volume?.level ?? 0) * 100)}%: exclusive mode works on this device, but playback is not bit-perfect and DSD is skipped`,
        );
      else if (output.soft_volume_enabled)
        showNotice("Software volume is on, and takes over in exclusive mode on devices without a hardware volume");
      else showNotice("Software volume off");
    } catch (e) {
      showError(e);
    }
  }

  /** Open the output; `null` if that failed. */
  async function applyOutput(): Promise<OutputSettings | null> {
    error = null;
    // Choosing an output calls off waiting for an unplugged device.
    waitingFor = null;
    try {
      const output = await api.setOutput(deviceId, wantedMode);
      takeOutput(output);
      return output;
    } catch (e) {
      showError(e);
      return null;
    }
  }

  /** What to tell the user about a device just opened, if anything stands out. */
  function outputNotice(output: OutputSettings, name: string): string | null {
    const parts: string[] = [];
    if (output.volume_lowered) parts.push("was at 100%, turned down to 10% to be safe");
    if (output.wanted === "exclusive" && !output.exclusive_available)
      parts.push("has no adjustable hardware volume, so shared mode is used (turn on Software volume for exclusive)");
    if (output.soft_volume) parts.push("has no hardware volume, so the software volume is used (not bit-perfect)");
    return parts.length ? `${name} ${parts.join("; ")}` : null;
  }

  /** Switch device, and say how loud the new one is set: each device keeps its own Windows volume. */
  async function changeDevice() {
    const output = await applyOutput();
    const volume = await volumeControl?.refresh();
    const name = devices.find((d) => d.id === deviceId)?.name ?? "System default";
    const notice = output && outputNotice(output, name);
    if (notice) showNotice(notice);
    else if (volume) showNotice(`${name}: volume ${Math.round(volume.level * 100)}%${volume.muted ? ", muted" : ""}`);
  }

  /** On focus: re-read the volume, and re-check exclusive if the device changed meanwhile (e.g. a new Windows default). */
  async function onWindowFocus() {
    const volume = await volumeControl?.refresh();
    // While an unplugged device is awaited, re-applying would give up on it.
    if (waitingFor !== null) return;
    // The software volume says nothing about the device: only compare a Windows volume.
    if (!volume?.software && (volume?.hardware ?? false) !== hardwareVolume) {
      await applyOutput();
      await volumeControl?.refresh();
    }
  }

  async function refreshDevices() {
    devices = await api.listDevices().catch((e) => (showError(e), []));
  }

  // ---- Playlists ----

  function replacePlaylist(updated: Playlist) {
    playlists = playlists.map((p) => (p.id === updated.id ? updated : p));
  }

  async function scan(id: number) {
    const run = (scanRun.get(id) ?? 0) + 1;
    scanRun.set(id, run);
    scanning.add(id);
    try {
      const found = await api.playlistScan(id);
      if (scanRun.get(id) === run) scanned[id] = found;
    } catch (e) {
      if (scanRun.get(id) === run) showError(e);
    } finally {
      if (scanRun.get(id) === run) scanning.delete(id);
    }
  }

  function openPlaylist(id: number) {
    view = id;
    renaming = null;
    if (!(id in scanned) && !scanning.has(id)) void scan(id);
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
      // A scan still running for it is dropped when it comes back.
      scanRun.delete(id);
      scanning.delete(id);
      delete sorts[id];
      saveSorts(sorts);
      view = "queue";
    } catch (e) {
      showError(e);
    }
  }

  /** Add folders and files to a playlist and refresh its track list. */
  async function addToPlaylist(id: number, paths: string[]): Promise<boolean> {
    try {
      const { playlist: updated, skipped } = await api.playlistAdd(id, paths);
      replacePlaylist(updated);
      if (skipped.length > 0) {
        const names = skipped.map((p) => p.split(/[\\/]/).pop()).join(", ");
        showError(`Not added, not a folder or a playable file: ${names}`);
      }
      // Rescan what is on screen; anything else rescans when next opened.
      if (view === id) await scan(id);
      else delete scanned[id];
      return skipped.length < paths.length;
    } catch (e) {
      showError(e);
      return false;
    }
  }

  async function pickFolders() {
    if (!playlist) return;
    const picked = await open({ directory: true, multiple: true, title: "Add folders to the playlist" });
    if (picked) await addToPlaylist(playlist.id, picked);
  }

  async function pickPlaylistFiles() {
    if (!playlist) return;
    const extensions = await api.supportedExtensions();
    const picked = await open({
      multiple: true,
      title: "Add files to the playlist",
      filters: [{ name: "Audio", extensions }],
    });
    if (picked) await addToPlaylist(playlist.id, picked);
  }

  /** Make a playlist holding `paths`, open it and start renaming it. */
  async function newPlaylistWith(paths: string[]) {
    const taken = new Set(playlists.map((p) => p.name.toLowerCase()));
    let name = "New playlist";
    for (let n = 2; taken.has(name.toLowerCase()); n++) name = `New playlist ${n}`;
    try {
      const created = await api.playlistCreate(name);
      playlists = [...playlists, created];
      view = created.id;
      await addToPlaylist(created.id, paths);
      renaming = name;
    } catch (e) {
      showError(e);
    }
  }

  /**
   * A column header was clicked in the open playlist: sort by it, reverse it,
   * or (third click, or the # column) go back to the playlist's own order.
   * Only the list and what Play starts change; a queue already playing keeps its order.
   */
  function sortPlaylist(key: SortKey | null) {
    if (!playlist) return;
    const next = key === null ? null : nextSort(playlistSort, key);
    if (next) sorts[playlist.id] = next;
    else delete sorts[playlist.id];
    saveSorts(sorts);
    // The rows moved, so an open track menu no longer belongs to its row.
    menu = null;
  }

  /** Remove single files from a playlist. Paths that came from a folder are left alone. */
  async function removeFromPlaylist(id: number, paths: string[]) {
    if (paths.length === 0) return;
    try {
      replacePlaylist(await api.playlistRemoveFiles(id, paths));
      await scan(id);
    } catch (e) {
      showError(e);
    }
  }

  /** A playlist was dragged in the sidebar: save the new order. */
  async function movePlaylists(rows: number[], slot: number) {
    const order = moveRows(playlists.length, rows, slot);
    if (isIdentity(order)) return;
    const ids = order.map((old) => playlists[old].id);
    playlists = order.map((old) => playlists[old]);
    try {
      playlists = await api.playlistReorder(ids);
    } catch (e) {
      showError(e);
      playlists = await api.playlists().catch(() => playlists);
    }
  }

  let navRows = $state<HTMLElement[]>([]);
  const playlistDrag = new RowDrag({
    rows: () => navRows.slice(0, playlists.length),
    scroller: () => navRows[0]?.closest("nav"),
    drop: (rows, slot) => void movePlaylists(rows, slot),
  });
  onDestroy(() => playlistDrag.destroy());

  // ---- Search ----

  /** Wait this long after the last key before searching. */
  const SEARCH_DELAY_MS = 150;
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  /** Searches started; a reply to an older one is dropped. */
  let searchRun = 0;

  /** The search box changed: search after a short pause, or leave search when it is empty. */
  function onSearchInput() {
    clearTimeout(searchTimer);
    if (searchQuery.trim() === "") {
      searchResults = null;
      if (view === "search") view = lastView;
      return;
    }
    if (view !== "search") {
      lastView = view;
      view = "search";
    }
    searchTimer = setTimeout(() => void runSearch(), SEARCH_DELAY_MS);
  }

  async function runSearch() {
    const query = searchQuery.trim();
    if (query === "") return;
    const run = ++searchRun;
    searching = true;
    try {
      const found = await api.search(query);
      if (run === searchRun) searchResults = found;
    } catch (e) {
      if (run === searchRun) showError(e);
    } finally {
      if (run === searchRun) searching = false;
    }
  }

  function clearSearch() {
    searchQuery = "";
    onSearchInput();
  }

  /** Open a view from the sidebar, leaving search. */
  function show(target: "queue" | number) {
    searchQuery = "";
    searchResults = null;
    if (target === "queue") view = "queue";
    else openPlaylist(target);
  }

  /** The transport's track title was clicked: open the queue at the track being heard. */
  async function revealCurrent() {
    if (currentIndex === null) return;
    show("queue");
    await tick();
    const row = document.querySelectorAll<HTMLElement>("main [data-row]")[currentIndex];
    row?.scrollIntoView({ block: "center", behavior: "smooth" });
  }

  // ---- Track menu ----

  /** "1 track", "3 tracks". */
  function count(n: number): string {
    return `${n} ${n === 1 ? "track" : "tracks"}`;
  }

  /**
   * The right-click menu for `rows` of `tracks`, the list on screen: one
   * track's menu, or one for the whole selection.
   */
  function trackMenu(tracks: TrackMeta[], rows: number[]): MenuItem[] {
    const picked = rows.map((i) => tracks[i]);
    const paths = picked.map((t) => t.path);
    const one = picked.length === 1 ? picked[0] : null;
    const inQueue = view === "queue";
    const here = playlist;
    const n = picked.length;

    const playlistItems: MenuItem[] = playlists.map((p) => {
      const already = paths.every((path) => holdsFile(p, path));
      return {
        label: p.name,
        disabled: already,
        hint: already ? "Already in" : undefined,
        action: async () => {
          if (await addToPlaylist(p.id, paths)) showNotice(`Added ${one ? "" : `${count(n)} `}to ${p.name}`);
        },
      };
    });
    if (playlistItems.length > 0) playlistItems.push("separator");
    playlistItems.push({ label: "New playlist…", action: () => newPlaylistWith(paths) });

    const items: MenuItem[] = [];
    if (inQueue) {
      items.push({ label: one ? "Play" : "Play from here", action: () => playQueue(rows[0]) });
      items.push({
        label: one ? "Remove from queue" : `Remove ${count(n)} from queue`,
        danger: true,
        action: () => removeFromQueue(rows),
      });
    } else {
      items.push({
        label: one ? "Play" : `Play ${count(n)}`,
        action: () => (one ? playTracks(tracks, rows[0]) : playTracks(picked, 0)),
      });
      items.push({
        label: one ? "Play next" : `Play ${count(n)} next`,
        action: async () => {
          if (await playNext(paths)) showNotice(one ? "Plays next" : `${count(n)} play next`);
        },
      });
      items.push({
        label: one ? "Add to queue" : `Add ${count(n)} to queue`,
        action: async () => {
          if (await addToQueue(paths)) showNotice(`Added ${one ? "" : `${count(n)} `}to the queue`);
        },
      });
    }
    items.push({ label: "Add to playlist", children: playlistItems });
    if (here) {
      const own = paths.filter((path) => holdsFile(here, path));
      const fromFolders = n - own.length;
      items.push({
        label: one || own.length <= 1 ? "Remove from playlist" : `Remove ${count(own.length)} from playlist`,
        danger: own.length > 0,
        disabled: own.length === 0,
        hint: fromFolders === 0 ? undefined : one || own.length === 0 ? "From a folder" : `${fromFolders} from folders stay`,
        action: () => removeFromPlaylist(here.id, own),
      });
    }
    if (one) {
      items.push("separator", { label: "Track info", action: () => (infoTrack = one) }, {
        label: "Show in File Explorer",
        action: () => api.showInFolder(one.path).catch(showError),
      });
    }
    return items;
  }

  function openTrackMenu(tracks: TrackMeta[], rows: number[], x: number, y: number) {
    if (rows.length === 0) return;
    menu = { x, y, items: trackMenu(tracks, rows) };
  }

  /** Keep the browser's own menu where it is useful: text fields and selected text. */
  function onContextMenu(event: MouseEvent) {
    const target = event.target as HTMLElement;
    const editable = target.closest("input, textarea, [contenteditable=true]");
    const selected = (window.getSelection()?.toString() ?? "").length > 0;
    if (!editable && !selected) event.preventDefault();
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
      takeOutput(output);
      const notice = output.missing_device
        ? `${output.missing_device} is not connected, so the system default is used for now`
        : outputNotice(output, "The output device");
      if (notice) showNotice(notice);
      playlists = await api.playlists();
      try {
        shuffle = localStorage.getItem("shuffle") === "1";
      } catch {}
      await api.setShuffle(shuffle);
      repeat = loadRepeat();
      await api.setRepeat(repeat);
      eq = await api.getEq();
      await refreshDevices();

      cleanups.push(
        await listenPlayer({
          state: (s) => (playState = s),
          track: (t) => {
            currentIndex = t.index;
            signalPath = t.signal_path;
          },
          current: (index) => (currentIndex = index),
          position: (p) => {
            placePosition(p.ms);
            if (p.duration_ms !== null || playState !== "stopped") durationMs = p.duration_ms;
          },
          error: showError,
          deviceLost: showError,
          queueEnded: () => placePosition(0, true),
        }),
      );
      cleanups.push(
        await listenOutput({
          waiting: (device) => {
            waitingFor = device;
            showError(`${device} was disconnected. Playback resumes when it is plugged back in.`);
            void refreshDevices();
          },
          reconnected: async ({ output, device }) => {
            waitingFor = null;
            error = null;
            takeOutput(output);
            await refreshDevices();
            await volumeControl?.refresh();
            showNotice(
              output.volume_lowered
                ? `${device} is back at 100%, turned down to 10% to be safe`
                : `${device} is back, playing again`,
            );
          },
        }),
      );
      cleanups.push(
        await getCurrentWebview().onDragDropEvent((event) => {
          if (event.payload.type === "over" || event.payload.type === "enter") dragging = true;
          else if (event.payload.type === "leave") dragging = false;
          else if (event.payload.type === "drop") {
            dragging = false;
            // On a playlist, a drop adds folders and files to it; elsewhere, it adds tracks to the queue.
            if (typeof view === "number") void addToPlaylist(view, event.payload.paths);
            else void addToQueue(event.payload.paths);
          }
        }),
      );
      cleanups.push(
        await listenLibrary((status) => {
          libraryTracks = status.tracks;
          // Playlists changed, or the launch refresh finished: the answer may be different now.
          if (view === "search") void runSearch();
        }),
      );
      libraryTracks = (await api.libraryStatus()).tracks;
    })().catch(showError);
    return () => cleanups.forEach((c) => c());
  });

  function onKey(event: KeyboardEvent) {
    const target = event.target as HTMLElement;
    // Track rows are buttons too, but Space there should play or pause, not "click" them.
    const typing = ["SELECT", "BUTTON", "INPUT", "TEXTAREA"].includes(target.tagName) && !target.hasAttribute("data-row");
    if (event.code === "Space" && !typing) {
      event.preventDefault();
      void togglePlay();
    } else if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "f") {
      event.preventDefault();
      searchInput?.focus();
      searchInput?.select();
    }
  }

  let searchInput = $state<HTMLInputElement>();

  /** Delete was pressed on rows of the list on screen. */
  function deleteRows(tracks: TrackMeta[], rows: number[]) {
    if (view === "queue") void removeFromQueue(rows);
    else if (playlist) {
      const here = playlist;
      const own = rows.map((i) => tracks[i].path).filter((path) => holdsFile(here, path));
      if (own.length === 0) showNotice("Tracks from a folder can only be removed with the folder");
      else void removeFromPlaylist(here.id, own);
    }
  }

  /** Focus and select an input as soon as it appears. */
  function focusOnMount(node: HTMLInputElement) {
    node.focus();
    node.select();
  }
</script>

<!-- Focus re-reads the volume, which may have been changed in Windows meanwhile. -->
<svelte:window bind:innerWidth onkeydown={onKey} oncontextmenu={onContextMenu} onfocus={() => void onWindowFocus()} />

<div class="flex h-full flex-col">
  <!-- Top bar: output device and mode -->
  <header class="flex items-center gap-3 border-b border-line bg-panel px-5 py-3">
    <div class="mr-auto flex items-center gap-2.5">
      <img src={appIcon} alt="" class="h-8 w-8" draggable="false" />
      <span class="text-[15px] font-semibold tracking-tight">MDMusic</span>
    </div>

    <label class="text-xs text-muted" for="device">Output</label>
    <select
      id="device"
      class="max-w-72 truncate rounded-lg border border-line bg-raised px-3 py-1.5 text-sm outline-none focus:border-accent"
      bind:value={deviceId}
      onchange={changeDevice}
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
          class="rounded-md px-3 py-1 capitalize transition-colors disabled:cursor-not-allowed disabled:opacity-40 {mode ===
          option
            ? 'bg-accent/20 text-accent'
            : 'text-muted enabled:hover:text-ink'}"
          disabled={option === "exclusive" && !exclusiveAvailable}
          title={option === "exclusive"
            ? !exclusiveAvailable
              ? "Unavailable: this device has no adjustable hardware volume, so in exclusive mode it would play at full volume. Turn on Software volume, or choose a device with a hardware volume."
              : hardwareVolume
                ? "Bit-perfect: MDMusic takes over the device, other apps are silent"
                : "MDMusic takes over the device, other apps are silent. The software volume sets the level, so it is not bit-perfect"
            : "Goes through the Windows mixer, other apps keep playing"}
          onclick={() => {
            wantedMode = option;
            void applyOutput().then(() => volumeControl?.refresh());
          }}
        >
          {option}
        </button>
      {/each}
    </div>

    {#if !hardwareVolume}
      <!-- Only for a device without a volume of its own: with one, the Windows volume keeps playback bit-perfect. -->
      <button
        class="flex items-center gap-1.5 rounded-lg border px-2.5 py-1.5 text-sm transition-colors {softVolumeEnabled
          ? 'border-warn/40 bg-warn/10 text-warn'
          : 'border-line text-muted hover:text-ink'}"
        aria-pressed={softVolumeEnabled}
        title={softVolumeEnabled
          ? "Software volume is on: in exclusive mode, MDMusic turns this device down by scaling the samples. Not bit-perfect, and DSD is skipped. Click to turn it off."
          : "This device has no volume of its own. Turn this on to use exclusive mode anyway: MDMusic then scales the samples to set the level, so it is not bit-perfect, and DSD is skipped."}
        onclick={toggleSoftVolume}
      >
        <span class="h-2 w-2 rounded-full {softVolumeEnabled ? 'bg-warn' : 'bg-line'}" aria-hidden="true"></span>
        Software volume
      </button>
    {/if}
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
      <div class="relative mb-2">
        <svg viewBox="0 0 24 24" class="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true">
          <circle cx="11" cy="11" r="7" /><path d="m20 20-3.5-3.5" />
        </svg>
        <input
          bind:this={searchInput}
          bind:value={searchQuery}
          oninput={onSearchInput}
          onkeydown={(e) => {
            if (e.key === "Escape") clearSearch();
          }}
          type="search"
          class="w-full rounded-lg border border-line bg-raised py-1.5 pl-8 pr-7 outline-none placeholder:text-muted focus:border-accent [&::-webkit-search-cancel-button]:hidden"
          placeholder={libraryTracks ? `Search ${libraryTracks.toLocaleString()} tracks` : "Search"}
          aria-label="Search all playlists"
          title="Search every playlist by title, artist, album, file or folder name (Ctrl+F)"
        />
        {#if searchQuery}
          <button
            class="absolute right-1.5 top-1/2 grid h-5 w-5 -translate-y-1/2 place-items-center rounded text-muted hover:text-ink"
            onclick={clearSearch}
            aria-label="Clear search"
          >
            ✕
          </button>
        {/if}
      </div>

      <button
        class="flex items-center justify-between rounded-lg px-3 py-2 text-left {view === 'queue'
          ? 'bg-raised font-medium text-ink'
          : 'text-muted hover:bg-raised hover:text-ink'}"
        onclick={() => show("queue")}
      >
        <span>Queue</span>
        {#if queue.length > 0}<span class="text-xs tabular-nums text-muted">{queue.length}</span>{/if}
      </button>

      <p class="px-3 pb-1 pt-4 text-xs font-medium uppercase tracking-wider text-muted">Playlists</p>
      {#each playlists as item, index (item.id)}
        {@const line = playlistDrag.line(index)}
        <button
          bind:this={navRows[index]}
          class="flex items-center gap-2 rounded-lg px-3 py-2 text-left {view === item.id
            ? 'bg-raised font-medium text-ink'
            : playlistDrag.active
              ? 'text-muted'
              : 'text-muted hover:bg-raised hover:text-ink'} {playlistDrag.active && playlistDrag.dragging.includes(index)
            ? 'opacity-40'
            : ''} {line === 'above'
            ? 'shadow-[inset_0_2px_0_var(--color-accent)]'
            : line === 'below'
              ? 'shadow-[inset_0_-2px_0_var(--color-accent)]'
              : ''}"
          onclick={() => {
            if (!playlistDrag.endedDrag()) show(item.id);
          }}
          onpointerdown={(e) => playlistDrag.down(e, [index])}
          onpointermove={(e) => playlistDrag.move(e)}
          onpointerup={(e) => playlistDrag.up(e)}
          onpointercancel={(e) => playlistDrag.cancel(e)}
          title="{item.name} (drag to reorder)"
        >
          <span class="min-w-0 flex-1 truncate">{item.name}</span>
          {#if scanning.has(item.id)}
            <span class="appear-late flex"><Spinner /></span>
          {/if}
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
      {#if view === "search"}
        <!-- Search: tracks of every playlist matching what was typed -->
        <div class="px-6 pb-3 pt-5">
          <!-- `min-h-8` holds the row's height while the button is away (nothing found yet). -->
          <div class="flex min-h-8 items-center gap-3">
            <h1 class="min-w-0 flex-1 truncate text-xl font-semibold">Search</h1>
            {#if searchResults && searchResults.length > 0}
              <button
                class="rounded-lg bg-accent px-3.5 py-1.5 text-sm font-medium text-base hover:brightness-110"
                onclick={() => searchResults && playTracks(searchResults, 0)}
              >
                Play all
              </button>
            {/if}
          </div>
          <p class="mt-1 min-h-4 text-xs text-muted" role="status">
            {#if searching && !searchResults}
              <span class="inline-flex items-center gap-1.5 align-middle"><Spinner /> Searching…</span>
            {:else if searchResults}
              {searchResults.length >= 500 ? "The first 500 tracks" : count(searchResults.length)} for “{searchQuery.trim()}”
              {#if libraryTracks !== null}· searched {libraryTracks.toLocaleString()} tracks in your playlists{/if}
            {/if}
          </p>
        </div>
        {#if searchResults && searchResults.length > 0}
          <TrackTable
            tracks={searchResults}
            currentPath={currentTrack?.path ?? null}
            playing={playState === "playing"}
            onplay={(index) => searchResults && playTracks(searchResults, index)}
            onmenu={(rows, x, y) => searchResults && openTrackMenu(searchResults, rows, x, y)}
          />
        {:else if searchResults}
          <div class="px-6 py-12 text-center text-sm text-muted">
            <p>Nothing matches. Every word has to appear in the title, artist, album, file or folder name.</p>
            {#if libraryTracks === 0}
              <p class="mt-1.5">Search looks through your playlists, and they have no tracks yet.</p>
            {/if}
          </div>
        {/if}
      {:else if playlist}
        <!-- A playlist: its folders and the tracks found in them -->
        <div class="px-6 pb-3 pt-5">
          <!-- The actions wrap under the name when the list is narrow, instead of pushing out of view. -->
          <div class="flex flex-wrap items-center gap-x-3 gap-y-2">
            {#if renaming !== null}
              <input
                class="min-w-0 flex-1 basis-40 rounded-lg border border-accent bg-raised px-3 py-1 text-xl font-semibold outline-none"
                bind:value={renaming}
                use:focusOnMount
                onblur={renamePlaylist}
                onkeydown={(e) => {
                  if (e.key === "Enter") void renamePlaylist();
                  else if (e.key === "Escape") renaming = null;
                }}
              />
            {:else}
              <h1 class="min-w-0 flex-1 basis-40 truncate text-xl font-semibold" title={playlist.name}>{playlist.name}</h1>
            {/if}
            <div class="flex flex-wrap gap-1 text-sm">
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
                class="rounded-lg px-2.5 py-1.5 text-muted hover:bg-raised hover:text-ink"
                onclick={pickPlaylistFiles}
              >
                Add files
              </button>
              <button
                class="rounded-lg px-2.5 py-1.5 text-muted hover:bg-raised hover:text-ink disabled:opacity-40"
                disabled={playlistLoading}
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

          <!-- `min-h-4` keeps the line's height while it is blank during the first load. -->
          <p class="mt-1 min-h-4 text-xs text-muted">
            {#if playlistLoading}
              <!-- The first load is shown in the list area below; this is a rescan over the old list. -->
              {#if playlistTracks && playlistTracks.length > 0}
                <span class="inline-flex items-center gap-1.5 align-middle" role="status">
                  <Spinner /> Scanning folders…
                </span>
              {/if}
            {:else if playlistTracks}
              {playlistTracks.length} {playlistTracks.length === 1 ? "track" : "tracks"}
              · {formatTime(playlistTracks.reduce((sum, t) => sum + (t.duration_ms ?? 0), 0))}
              {#if playlist.files.length > 0}
                <!-- Rows mark only single files; folder tracks, the usual case, go unmarked. -->
                <span class="ml-3 inline-flex items-center gap-1.5 align-middle">
                  <SourceMark kind="file" /> added as a single file
                </span>
              {/if}
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

        {#if playlist.folders.length === 0 && playlist.files.length === 0}
          <div class="grid place-items-center px-8 py-16 text-center">
            <div class="max-w-sm">
              <p class="text-lg font-medium">This playlist is empty</p>
              <p class="mt-1.5 text-sm text-muted">
                Add a folder and every audio file inside it, subfolders included, becomes part of the playlist. Add
                single tracks with Add files, by dropping them here, or by right-clicking any track.
              </p>
              <div class="mt-5 flex justify-center gap-2">
                <button
                  class="rounded-lg bg-accent px-4 py-2 text-sm font-medium text-base hover:brightness-110"
                  onclick={pickFolders}
                >
                  Add folder
                </button>
                <button
                  class="rounded-lg border border-line px-4 py-2 text-sm font-medium hover:bg-raised"
                  onclick={pickPlaylistFiles}
                >
                  Add files
                </button>
              </div>
            </div>
          </div>
        {:else if playlistTracks && playlistTracks.length > 0}
          <TrackTable
            tracks={playlistTracks}
            currentPath={currentTrack?.path ?? null}
            playing={playState === "playing"}
            onplay={(index) => playTracks(playlistTracks, index)}
            onmenu={(rows, x, y) => openTrackMenu(playlistTracks, rows, x, y)}
            ondelete={(rows) => deleteRows(playlistTracks, rows)}
            source={(track) => (playlist && holdsFile(playlist, track.path) ? "file" : "folder")}
            sort={playlistSort}
            onsort={sortPlaylist}
          />
        {:else if playlistLoading}
          <div class="appear-late flex flex-col items-center gap-3 px-6 py-16 text-sm text-muted" role="status">
            <Spinner class="h-6 w-6" />
            <p>Loading tracks…</p>
          </div>
        {:else if playlistTracks}
          <p class="px-6 py-10 text-center text-sm text-muted">No audio files were found for this playlist.</p>
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
        <!-- Same header shape as a playlist: name and actions, then a count line. -->
        <div class="flex flex-wrap items-center justify-between gap-x-3 gap-y-2 px-6 pt-5">
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
        <p class="mt-1 px-6 pb-3 text-xs text-muted">
          {count(queue.length)} · {formatTime(queue.reduce((sum, t) => sum + (t.duration_ms ?? 0), 0))}
        </p>
        <TrackTable
          tracks={queue}
          currentPath={currentTrack?.path ?? null}
          playing={playState === "playing"}
          onplay={playQueue}
          onmenu={(rows, x, y) => openTrackMenu(queue, rows, x, y)}
          ondelete={(rows) => deleteRows(queue, rows)}
          onmove={moveInQueue}
        />
      {/if}

      {#if dragging}
        <div class="pointer-events-none absolute inset-3 grid place-items-center rounded-2xl border-2 border-dashed border-accent bg-accent/10">
          <p class="font-medium text-accent">
            {playlist ? `Drop folders or files to add them to "${playlist.name}"` : "Drop to add to the queue"}
          </p>
        </div>
      {/if}

      {#if notice}
        <div
          class="pointer-events-none sticky bottom-4 mx-auto w-fit rounded-full border border-line bg-raised px-4 py-2 text-sm shadow-lg shadow-black/40"
          role="status"
        >
          {notice}
        </div>
      {/if}
    </main>

    {#if showPanel}
      <NowPlaying
        track={currentTrack}
        {positionMs}
        onseek={seekTo}
        {disc}
        playing={playState === "playing"}
        ontoggledisc={toggleDisc}
      />
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
        step="any"
        value={shownMs}
        style="--fill: {fill}%"
        disabled={!durationMs || playState === "stopped"}
        aria-label="Seek"
        oninput={(e) => (scrubMs = Math.round(Number(e.currentTarget.value)))}
        onchange={commitSeek}
      />
      <span class="w-12 text-xs tabular-nums text-muted">{durationMs ? formatTime(durationMs) : "–:––"}</span>
    </div>

    <div class="mt-3 grid grid-cols-[1fr_auto_1fr] items-center gap-4">
      <div class="flex min-w-0 items-center gap-3">
        <Cover
          path={currentTrack?.path ?? null}
          class="h-11 w-11 shrink-0 rounded-md"
          {disc}
          spinning={playState === "playing"}
          onclick={toggleDisc}
          title={disc ? "Click for the square cover" : "Click to spin the cover like a record"}
        />
        {#if currentTrack}
          <button class="group min-w-0 text-left" title="Show in the queue" onclick={revealCurrent}>
            <p class="truncate text-sm font-medium group-hover:underline">{currentTrack.title}</p>
            {#if currentTrack.artist}
              <p class="mt-0.5 truncate text-xs text-muted">{currentTrack.artist}</p>
            {/if}
          </button>
        {:else}
          <p class="min-w-0 truncate text-sm font-medium text-muted">Not playing</p>
        {/if}
      </div>

      <div class="flex items-center gap-2">
        <button
          class="relative grid h-9 w-9 place-items-center rounded-full hover:bg-raised {shuffle
            ? 'text-accent'
            : 'text-muted hover:text-ink'}"
          onclick={toggleShuffle}
          aria-pressed={shuffle}
          aria-label="Shuffle"
          title={shuffle ? "Shuffle is on: next plays a random track" : "Shuffle is off"}
        >
          <svg viewBox="0 0 24 24" class="h-[18px] w-[18px]" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M16 3h5v5M4 20 21 3M21 16v5h-5M15 15l6 6M4 4l5 5" />
          </svg>
          {#if shuffle}<span class="absolute bottom-0.5 h-1 w-1 rounded-full bg-accent"></span>{/if}
        </button>
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
        <button
          class="relative grid h-9 w-9 place-items-center rounded-full hover:bg-raised {repeat !== 'off'
            ? 'text-accent'
            : 'text-muted hover:text-ink'}"
          onclick={cycleRepeat}
          aria-label="Repeat: {repeat}"
          title={REPEAT_TITLE[repeat]}
        >
          <svg viewBox="0 0 24 24" class="h-[18px] w-[18px]" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="m17 2 4 4-4 4M3 11V10a4 4 0 0 1 4-4h14M7 22l-4-4 4-4M21 13v1a4 4 0 0 1-4 4H3" />
          </svg>
          {#if repeat === "one"}
            <span class="absolute right-1 top-1 grid h-3.5 w-3.5 place-items-center rounded-full bg-accent text-[9px] font-bold leading-none text-base">1</span>
          {/if}
          {#if repeat !== "off"}<span class="absolute bottom-0.5 h-1 w-1 rounded-full bg-accent"></span>{/if}
        </button>
      </div>

      <!-- Signal path: what the file is, what the device receives -->
      <div class="@container flex min-w-0 items-center justify-end gap-2 text-xs">
        {#if signalPath && playState !== "stopped"}
          <!-- When space runs out, the file side gives way; what the DAC receives stays. Below
               28rem (the controls beside it take about 27rem) there is no room for either, and
               the badge's tooltip carries the full path instead. -->
          <span class="hidden min-w-0 overflow-hidden whitespace-nowrap text-muted @md:flex" title={signalTitle}>
            <span class="truncate">{sourceLabel}</span><span class="shrink-0">&nbsp;→ {outputLabel}</span>
          </span>
          <span
            class="shrink-0 rounded-full px-2.5 py-1 font-medium {signalPath.bit_perfect
              ? 'bg-good/15 text-good'
              : 'bg-warn/15 text-warn'}"
            title={badgeTitle}
          >
            {signalPath.bit_perfect
              ? "Bit-perfect"
              : signalPath.eq
                ? "EQ on"
                : signalPath.soft_volume
                  ? "Soft volume"
                  : signalPath.exclusive
                    ? "Exclusive"
                    : "Shared"}
          </span>
        {/if}
        <button
          class="relative grid h-8 shrink-0 place-items-center rounded-lg px-2 text-xs font-semibold tracking-wide hover:bg-raised disabled:opacity-40 {showEq ||
          eq?.enabled
            ? 'text-accent'
            : 'text-muted hover:text-ink'}"
          disabled={eq === null}
          aria-pressed={showEq}
          title={eq?.enabled ? "Equalizer is on: PCM is not bit-perfect" : "Equalizer (off)"}
          onclick={() => (showEq = !showEq)}
        >
          EQ
          {#if eq?.enabled}<span class="absolute bottom-0.5 h-1 w-1 rounded-full bg-accent"></span>{/if}
        </button>
        <VolumeControl bind:this={volumeControl} exclusive={mode === "exclusive"} onerror={showError} />
        <button
          class="grid h-8 w-8 shrink-0 place-items-center rounded-lg hover:bg-raised {showPanel
            ? 'text-accent'
            : 'text-muted hover:text-ink'}"
          title={showPanel ? "Hide cover and lyrics" : "Show cover and lyrics"}
          aria-pressed={showPanel}
          onclick={() => {
            showPanel = !showPanel;
            panelAutoHidden = false;
          }}
        >
          <svg viewBox="0 0 24 24" class="h-4.5 w-4.5" fill="none" stroke="currentColor" stroke-width="2">
            <rect x="3" y="4" width="18" height="16" rx="2" /><path d="M15 4v16" />
          </svg>
        </button>
      </div>
    </div>
  </footer>
</div>

{#if showEq && eq}
  <EqPanel bind:settings={eq} onclose={() => (showEq = false)} onerror={showError} />
{/if}

{#if infoTrack}
  <TrackInfo track={infoTrack} onclose={() => (infoTrack = null)} onerror={showError} />
{/if}

{#if menu}
  <ContextMenu x={menu.x} y={menu.y} items={menu.items} onclose={() => (menu = null)} />
{/if}
