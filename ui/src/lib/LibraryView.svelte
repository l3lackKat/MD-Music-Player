<script lang="ts" module>
  import type { AlbumSummary } from "./api";

  /**
   * What the library side of the main area shows: every album, every
   * artist, or one of either. `back` is where the back link goes.
   */
  export type Browse =
    | { kind: "albums" }
    | { kind: "artists" }
    | { kind: "album"; album: AlbumSummary; back: Browse }
    | { kind: "artist"; name: string; back: Browse };

  export function isBrowse(view: unknown): view is Browse {
    return typeof view === "object" && view !== null && "kind" in view;
  }

  /** The grid a page was reached from, for the sidebar to mark. */
  export function browseRoot(view: Browse): "albums" | "artists" {
    let v = view;
    while (v.kind === "album" || v.kind === "artist") v = v.back;
    return v.kind;
  }
</script>

<script lang="ts">
  // Albums and artists from the library index (every playlist's tracks).
  // The grids open an album or an artist; their pages list the tracks in a
  // TrackTable, so playing, the track menu and selection work as anywhere.
  import { api, formatTime, type ArtistDetail, type ArtistSummary, type TrackMeta } from "./api";
  import Spinner from "./Spinner.svelte";
  import Thumb from "./Thumb.svelte";
  import TrackTable from "./TrackTable.svelte";

  let {
    view,
    version,
    currentPath,
    playing,
    onnavigate,
    onplay,
    onqueue,
    onmenu,
    onerror,
  }: {
    view: Browse;
    /** Bumped when the library index changed: lists are read again. */
    version: number;
    currentPath: string | null;
    playing: boolean;
    onnavigate: (view: Browse) => void;
    /** Replace the queue with `tracks` and play from `index`. */
    onplay: (tracks: TrackMeta[], index: number) => void;
    onqueue: (paths: string[]) => void;
    onmenu: (tracks: TrackMeta[], rows: number[], x: number, y: number) => void;
    onerror: (e: unknown) => void;
  } = $props();

  let albums = $state<AlbumSummary[] | null>(null);
  let artists = $state<ArtistSummary[] | null>(null);
  /** The open album's tracks, for `albumFor`. */
  let albumTracks = $state<TrackMeta[] | null>(null);
  let albumFor = $state<string | null>(null);
  /** The open artist, for `artistFor`. */
  let artist = $state<ArtistDetail | null>(null);
  let artistFor = $state<string | null>(null);
  let missing = $state(false);

  // Read what the view needs, again whenever the index changes. Lists stay
  // on screen while they are read again; a different album or artist starts empty.
  $effect(() => {
    const v = view;
    void version;
    let stale = false;
    missing = false;
    const fail = (e: unknown) => !stale && onerror(e);
    if (v.kind === "albums") {
      api.albums().then((r) => !stale && (albums = r)).catch(fail);
    } else if (v.kind === "artists") {
      api.artists().then((r) => !stale && (artists = r)).catch(fail);
    } else if (v.kind === "album") {
      if (albumFor !== v.album.id) albumTracks = null;
      albumFor = v.album.id;
      api.albumTracks(v.album.id).then((r) => !stale && (albumTracks = r)).catch(fail);
    } else {
      if (artistFor !== v.name) artist = null;
      artistFor = v.name;
      api
        .artist(v.name)
        .then((r) => {
          if (stale) return;
          artist = r;
          missing = r === null;
        })
        .catch(fail);
    }
    return () => {
      stale = true;
    };
  });

  // A new page starts at the top.
  $effect(() => {
    void view;
    document.querySelector("main")?.scrollTo({ top: 0 });
  });

  async function playAlbum(album: AlbumSummary) {
    try {
      const tracks = await api.albumTracks(album.id);
      if (tracks.length > 0) onplay(tracks, 0);
    } catch (e) {
      onerror(e);
    }
  }

  function openAlbum(album: AlbumSummary) {
    onnavigate({ kind: "album", album, back: view });
  }

  function openArtist(name: string) {
    onnavigate({ kind: "artist", name, back: view });
  }

  function backLabel(back: Browse): string {
    switch (back.kind) {
      case "albums":
        return "Albums";
      case "artists":
        return "Artists";
      case "album":
        return back.album.title;
      case "artist":
        return back.name;
    }
  }

  /** "1 album", "3 albums". */
  function plural(n: number, word: string): string {
    return `${n.toLocaleString()} ${word}${n === 1 ? "" : "s"}`;
  }

  function totalTime(tracks: TrackMeta[]): string {
    return formatTime(tracks.reduce((sum, t) => sum + (t.duration_ms ?? 0), 0));
  }

  const BUTTON_PRIMARY = "rounded-lg bg-accent px-3.5 py-1.5 text-sm font-medium text-base hover:brightness-110";
  const BUTTON_QUIET = "rounded-lg px-2.5 py-1.5 text-sm text-muted hover:bg-raised hover:text-ink";
</script>

{#snippet back(target: Browse)}
  <button class="mb-3 flex items-center gap-1 text-sm text-muted hover:text-ink" onclick={() => onnavigate(target)}>
    <svg viewBox="0 0 24 24" class="h-4 w-4" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="m15 18-6-6 6-6" /></svg>
    {backLabel(target)}
  </button>
{/snippet}

{#snippet albumGrid(list: AlbumSummary[], showArtist: boolean)}
  <ul class="grid grid-cols-[repeat(auto-fill,minmax(9.5rem,1fr))] gap-x-4 gap-y-6">
    {#each list as album (album.id)}
      <!-- `@container`: the cover is as tall as the tile is wide (100cqw), which places the play button on it. -->
      <li class="group @container relative min-w-0">
        <button class="block w-full text-left" title={album.title} onclick={() => openAlbum(album)}>
          <Thumb
            path={album.cover_path}
            size="grid"
            class="aspect-square w-full rounded-lg shadow-md shadow-black/30 transition-[filter] group-hover:brightness-90"
          />
          <p class="mt-2 truncate text-sm font-medium">{album.title}</p>
          <p class="mt-0.5 truncate text-xs text-muted">
            {#if showArtist}{album.artist ?? "Various artists"}{#if album.year}{` · ${album.year}`}{/if}{:else}{album.year ??
                plural(album.track_count, "track")}{/if}
          </p>
        </button>
        <!-- Play straight from the grid. A sibling of the tile, not inside it: a button may not hold a button. -->
        <button
          class="absolute right-2 top-[calc(100cqw-3rem)] grid h-10 w-10 place-items-center rounded-full bg-accent text-base opacity-0 shadow-lg shadow-black/40 transition-opacity hover:brightness-110 focus-visible:opacity-100 group-hover:opacity-100"
          title="Play {album.title}"
          aria-label="Play {album.title}"
          onclick={() => playAlbum(album)}
        >
          <svg viewBox="0 0 24 24" class="h-4 w-4" fill="currentColor" aria-hidden="true"><path d="M7 4.5v15l12.5-7.5z" /></svg>
        </button>
      </li>
    {/each}
  </ul>
{/snippet}

{#snippet loadingNote(text: string)}
  <div class="appear-late flex flex-col items-center gap-3 px-6 py-16 text-sm text-muted" role="status">
    <Spinner class="h-6 w-6" />
    <p>{text}</p>
  </div>
{/snippet}

{#snippet emptyNote(title: string, text: string)}
  <div class="grid place-items-center px-8 py-16 text-center">
    <div class="max-w-sm">
      <p class="text-lg font-medium">{title}</p>
      <p class="mt-1.5 text-sm text-muted">{text}</p>
    </div>
  </div>
{/snippet}

{#if view.kind === "albums"}
  <div class="px-6 pb-8 pt-5">
    <h1 class="text-xl font-semibold">Albums</h1>
    <p class="mt-1 min-h-4 text-xs text-muted">{albums ? plural(albums.length, "album") : ""}</p>
    <div class="mt-4">
      {#if albums === null}
        {@render loadingNote("Loading albums…")}
      {:else if albums.length === 0}
        {@render emptyNote(
          "No albums yet",
          "Albums are gathered from the album tags of the tracks in your playlists. Add a folder to a playlist and they show up here.",
        )}
      {:else}
        {@render albumGrid(albums, true)}
      {/if}
    </div>
  </div>
{:else if view.kind === "artists"}
  <div class="px-6 pb-8 pt-5">
    <h1 class="text-xl font-semibold">Artists</h1>
    <p class="mt-1 min-h-4 text-xs text-muted">{artists ? plural(artists.length, "artist") : ""}</p>
    <div class="mt-4">
      {#if artists === null}
        {@render loadingNote("Loading artists…")}
      {:else if artists.length === 0}
        {@render emptyNote(
          "No artists yet",
          "Artists are gathered from the tags of the tracks in your playlists. Add a folder to a playlist and they show up here.",
        )}
      {:else}
        <ul class="grid grid-cols-[repeat(auto-fill,minmax(8.5rem,1fr))] gap-x-4 gap-y-6">
          {#each artists as item (item.name)}
            <li class="min-w-0">
              <button class="group block w-full text-center" title={item.name} onclick={() => openArtist(item.name)}>
                <Thumb
                  path={item.cover_path}
                  size="grid"
                  class="mx-auto aspect-square w-full rounded-full shadow-md shadow-black/30 transition-[filter] group-hover:brightness-90"
                />
                <p class="mt-2 truncate text-sm font-medium">{item.name}</p>
                <p class="mt-0.5 truncate text-xs text-muted">
                  {item.album_count > 0 ? `${plural(item.album_count, "album")} · ` : ""}{plural(item.track_count, "track")}
                </p>
              </button>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  </div>
{:else if view.kind === "album"}
  {@const album = view.album}
  <div class="px-6 pb-3 pt-5">
    {@render back(view.back)}
    <div class="flex flex-wrap items-end gap-5">
      <Thumb path={album.cover_path} size="grid" class="h-36 w-36 rounded-lg shadow-lg shadow-black/40" />
      <div class="min-w-0 flex-1 basis-60">
        <p class="text-xs font-medium uppercase tracking-wider text-muted">Album</p>
        <h1 class="mt-1 truncate text-2xl font-semibold" title={album.title}>{album.title}</h1>
        <p class="mt-1 truncate text-sm">
          {#if album.artist}
            <button class="font-medium hover:underline" onclick={() => album.artist && openArtist(album.artist)}>
              {album.artist}
            </button>
          {:else}
            <span class="text-muted">Various artists</span>
          {/if}
          {#if album.year}<span class="text-muted"> · {album.year}</span>{/if}
        </p>
        <p class="mt-1 text-xs text-muted">
          {plural(albumTracks?.length ?? album.track_count, "track")} · {albumTracks
            ? totalTime(albumTracks)
            : formatTime(album.duration_ms)}
        </p>
        <div class="mt-3 flex flex-wrap gap-1">
          <button
            class="{BUTTON_PRIMARY} disabled:opacity-40"
            disabled={!albumTracks || albumTracks.length === 0}
            onclick={() => albumTracks && onplay(albumTracks, 0)}
          >
            Play
          </button>
          <button
            class={BUTTON_QUIET}
            disabled={!albumTracks}
            onclick={() => albumTracks && onqueue(albumTracks.map((t) => t.path))}
          >
            Add to queue
          </button>
        </div>
      </div>
    </div>
  </div>
  {#if albumTracks === null}
    {@render loadingNote("Loading tracks…")}
  {:else if albumTracks.length === 0}
    <p class="px-6 py-10 text-center text-sm text-muted">None of this album's files can be found any more.</p>
  {:else}
    <TrackTable
      tracks={albumTracks}
      {currentPath}
      {playing}
      onplay={(index) => albumTracks && onplay(albumTracks, index)}
      onmenu={(rows, x, y) => albumTracks && onmenu(albumTracks, rows, x, y)}
    />
  {/if}
{:else}
  <div class="px-6 pb-3 pt-5">
    {@render back(view.back)}
    {#if missing}
      {@render emptyNote("Artist not found", `No track in your playlists is by ${view.name} any more.`)}
    {:else}
      <div class="flex flex-wrap items-end gap-5">
        {#if artist}
          <Thumb
            path={artist.albums[0]?.cover_path ?? artist.tracks[0]?.path ?? ""}
            size="grid"
            class="h-32 w-32 rounded-full shadow-lg shadow-black/40"
          />
        {:else}
          <span class="h-32 w-32 shrink-0 rounded-full bg-raised"></span>
        {/if}
        <div class="min-w-0 flex-1 basis-60">
          <p class="text-xs font-medium uppercase tracking-wider text-muted">Artist</p>
          <h1 class="mt-1 truncate text-2xl font-semibold" title={artist?.name ?? view.name}>{artist?.name ?? view.name}</h1>
          <p class="mt-1 min-h-4 text-xs text-muted">
            {#if artist}
              {artist.albums.length > 0 ? `${plural(artist.albums.length, "album")} · ` : ""}{plural(
                artist.tracks.length,
                "track",
              )} · {totalTime(artist.tracks)}
            {/if}
          </p>
          <div class="mt-3 flex flex-wrap gap-1">
            <button
              class="{BUTTON_PRIMARY} disabled:opacity-40"
              disabled={!artist || artist.tracks.length === 0}
              onclick={() => artist && onplay(artist.tracks, 0)}
            >
              Play all
            </button>
            <button
              class={BUTTON_QUIET}
              disabled={!artist}
              onclick={() => artist && onqueue(artist.tracks.map((t) => t.path))}
            >
              Add to queue
            </button>
          </div>
        </div>
      </div>
      {#if artist === null}
        {@render loadingNote("Loading…")}
      {:else}
        {#if artist.albums.length > 0}
          <h2 class="mb-3 mt-8 text-xs font-medium uppercase tracking-wider text-muted">Albums</h2>
          {@render albumGrid(artist.albums, false)}
        {/if}
        <h2 class="mb-2 mt-8 text-xs font-medium uppercase tracking-wider text-muted">Tracks</h2>
      {/if}
    {/if}
  </div>
  {#if artist && artist.tracks.length > 0}
    <TrackTable
      tracks={artist.tracks}
      {currentPath}
      {playing}
      onplay={(index) => artist && onplay(artist.tracks, index)}
      onmenu={(rows, x, y) => artist && onmenu(artist.tracks, rows, x, y)}
    />
  {/if}
{/if}
