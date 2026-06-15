<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';

  type AlbumBrief = {
    cid: string;
    name: string;
    coverUrl: string;
    artistes?: string[];
  };

  type SongBrief = {
    cid: string;
    name: string;
    artistes?: string[];
  };

  type AlbumDetail = AlbumBrief & {
    intro: string;
    belong: string;
    coverDeUrl: string;
    songs: SongBrief[];
  };

  type AlbumDownloadReport = {
    albumName: string;
    totalTracks: number;
    tracks: TrackDownloadReport[];
    issues: DownloadIssue[];
  };

  type TrackDownloadReport = {
    index: number;
    name: string;
    status: SongStatus;
  };

  type DownloadIssue = {
    kind: string;
    item: string;
    message: string;
  };

  type SongStatus = 'queued' | 'checking' | 'getting' | 'resuming' | 'tagging' | 'skipped' | 'done' | 'failed';

  type DownloadEvent =
    | { type: 'albumStarted'; albumName: string; totalTracks: number }
    | { type: 'trackQueued'; index: number; name: string }
    | { type: 'trackStatus'; index: number; name: string; status: SongStatus }
    | { type: 'trackProgress'; index: number; name: string; downloaded: number; total: number; speedBps: number; resumed: boolean }
    | { type: 'trackFinished'; index: number; name: string; status: SongStatus }
    | { type: 'issue'; issue: DownloadIssue }
    | { type: 'albumFinished'; report: AlbumDownloadReport }
    | { type: 'albumFailed'; error: string };

  let albums = $state<AlbumBrief[]>([]);
  let activeAlbum = $state<AlbumDetail | null>(null);
  let selectedCid = $state<string | null>(null);
  let loadingAlbums = $state(false);
  let loadingDetail = $state(false);
  let downloading = $state(false);
  let status = $state('准备加载 Monster Siren 专辑。');
  let error = $state<string | null>(null);
  let currentTrack = $state<string | null>(null);
  let downloadPercent = $state(0);
  let downloadIssues = $state<DownloadIssue[]>([]);

  const visibleArtists = (artists?: string[]) => artists?.join(' / ') || 'Unknown artist';

  function applyDownloadEvent(event: DownloadEvent) {
    switch (event.type) {
      case 'albumStarted':
        downloading = true;
        error = null;
        currentTrack = null;
        downloadPercent = 0;
        downloadIssues = [];
        status = `正在下载 ${event.albumName}...`;
        break;
      case 'trackQueued':
      case 'trackStatus':
        currentTrack = event.name;
        break;
      case 'trackProgress':
        currentTrack = event.name;
        downloadPercent = event.total > 0 ? Math.round((event.downloaded / event.total) * 100) : 0;
        break;
      case 'trackFinished':
        currentTrack = event.name;
        if (event.status === 'failed') {
          error = `${event.name} 下载失败。`;
        }
        break;
      case 'issue':
        downloadIssues = [...downloadIssues, event.issue];
        break;
      case 'albumFinished': {
        const downloadedTracks = event.report.tracks.filter((track) => track.status === 'done').length;
        const failedTracks = event.report.tracks.filter((track) => track.status === 'failed').length;
        const issueText = event.report.issues.length > 0 ? `，附带 ${event.report.issues.length} 个问题` : '';
        downloading = false;
        downloadPercent = 100;
        status = `${event.report.albumName} 下载完成：${downloadedTracks}/${event.report.totalTracks} 首，失败 ${failedTracks} 首${issueText}。`;
        break;
      }
      case 'albumFailed':
        downloading = false;
        if (event.error.toLowerCase().includes('cancelled')) {
          error = null;
          status = '下载已取消。';
        } else {
          error = event.error;
          status = '下载失败。';
        }
        break;
    }
  }

  onMount(() => {
    const unlisten = listen<DownloadEvent>('download-event', (event) => {
      applyDownloadEvent(event.payload);
    });

    return () => {
      void unlisten.then((dispose) => dispose());
    };
  });

  async function loadAlbums() {
    loadingAlbums = true;
    error = null;
    status = '正在加载专辑列表...';
    try {
      albums = await invoke<AlbumBrief[]>('list_albums');
      status = `已加载 ${albums.length} 张专辑。`;
      if (albums[0]) {
        await selectAlbum(albums[0].cid);
      }
    } catch (err) {
      error = String(err);
      status = '专辑列表加载失败。';
    } finally {
      loadingAlbums = false;
    }
  }

  async function selectAlbum(cid: string) {
    selectedCid = cid;
    loadingDetail = true;
    error = null;
    try {
      activeAlbum = await invoke<AlbumDetail>('get_album_detail', { cid });
      status = `已选择 ${activeAlbum.name}。`;
    } catch (err) {
      activeAlbum = null;
      error = String(err);
      status = '专辑详情加载失败。';
    } finally {
      loadingDetail = false;
    }
  }

  async function downloadActiveAlbum() {
    if (!activeAlbum || downloading) return;
    downloading = true;
    error = null;
    currentTrack = null;
    downloadPercent = 0;
    downloadIssues = [];
    status = `正在下载 ${activeAlbum.name}...`;
    try {
      await invoke<void>('download_album', { cid: activeAlbum.cid });
    } catch (err) {
      error = String(err);
      status = '下载失败。';
      downloading = false;
    }
  }

  async function cancelDownload() {
    if (!downloading) return;
    try {
      await invoke<void>('cancel_download');
      status = '正在取消下载...';
    } catch (err) {
      error = String(err);
    }
  }
</script>

<main class="min-h-screen bg-graphite text-zinc-100">
  <section class="mx-auto flex min-h-screen w-full max-w-7xl flex-col px-4 py-5 sm:px-6 lg:px-8">
    <header class="flex flex-col gap-4 border-b border-white/10 pb-5 md:flex-row md:items-end md:justify-between">
      <div>
        <p class="text-xs font-semibold uppercase tracking-[0.35em] text-amberline">MSR Downloader</p>
        <h1 class="mt-2 text-3xl font-semibold tracking-tight text-white sm:text-4xl">Monster Siren GUI</h1>
        <p class="mt-2 max-w-2xl text-sm leading-6 text-zinc-400">
          与现有 CLI、TUI 并列的 Tauri 桌面入口。当前提供专辑浏览、详情预览和整专下载。
        </p>
      </div>
      <button
        class="inline-flex items-center justify-center rounded-md border border-amberline/60 px-4 py-2 text-sm font-medium text-amberline transition hover:bg-amberline hover:text-graphite focus:outline-none focus:ring-2 focus:ring-amberline focus:ring-offset-2 focus:ring-offset-graphite disabled:cursor-not-allowed disabled:opacity-50"
        onclick={loadAlbums}
        disabled={loadingAlbums}
      >
        {loadingAlbums ? '加载中...' : '刷新专辑'}
      </button>
    </header>

    <div class="grid flex-1 gap-5 py-5 lg:grid-cols-[360px_1fr]">
      <aside class="min-h-[18rem] rounded-xl border border-white/10 bg-panel/95 p-3 shadow-2xl shadow-black/20">
        <div class="mb-3 flex items-center justify-between px-1">
          <h2 class="text-sm font-semibold text-zinc-200">专辑</h2>
          <span class="text-xs text-zinc-500">{albums.length} items</span>
        </div>

        {#if albums.length === 0 && !loadingAlbums}
          <div class="rounded-lg border border-dashed border-white/15 p-5 text-sm leading-6 text-zinc-400">
            尚未加载专辑。点击右上角刷新开始。
          </div>
        {:else}
          <div class="max-h-[calc(100vh-14rem)] space-y-2 overflow-y-auto pr-1">
            {#each albums as album}
              <button
                class="grid w-full grid-cols-[3.5rem_1fr] gap-3 rounded-lg border p-2 text-left transition focus:outline-none focus:ring-2 focus:ring-amberline {selectedCid === album.cid ? 'border-amberline/60 bg-amberline/10' : 'border-white/10 bg-white/[0.03] hover:border-white/20 hover:bg-white/[0.06]'}"
                onclick={() => selectAlbum(album.cid)}
              >
                <img class="h-14 w-14 rounded-md object-cover" src={album.coverUrl} alt="{album.name} cover" />
                <span class="min-w-0">
                  <span class="block truncate text-sm font-medium text-zinc-100">{album.name}</span>
                  <span class="mt-1 block truncate text-xs text-zinc-500">{visibleArtists(album.artistes)}</span>
                </span>
              </button>
            {/each}
          </div>
        {/if}
      </aside>

      <section class="rounded-xl border border-white/10 bg-panel/95 p-4 shadow-2xl shadow-black/20 sm:p-6">
        {#if loadingDetail}
          <div class="space-y-4" aria-busy="true" aria-label="Loading album detail">
            <div class="h-48 rounded-xl bg-white/10"></div>
            <div class="h-8 w-2/3 rounded bg-white/10"></div>
            <div class="h-4 w-full rounded bg-white/10"></div>
            <div class="h-4 w-5/6 rounded bg-white/10"></div>
          </div>
        {:else if activeAlbum}
          <div class="grid gap-6 xl:grid-cols-[18rem_1fr]">
            <img class="aspect-square w-full rounded-xl object-cover ring-1 ring-white/10" src={activeAlbum.coverDeUrl || activeAlbum.coverUrl} alt="{activeAlbum.name} cover" />
            <div class="min-w-0">
              <p class="text-xs font-medium uppercase tracking-[0.25em] text-zinc-500">{activeAlbum.belong || 'Monster Siren Records'}</p>
              <h2 class="mt-2 text-3xl font-semibold tracking-tight text-white">{activeAlbum.name}</h2>
              <p class="mt-3 max-w-3xl whitespace-pre-line text-sm leading-6 text-zinc-400">{activeAlbum.intro || '暂无专辑介绍。'}</p>

              <div class="mt-5 flex flex-wrap items-center gap-3">
                <button
                  class="rounded-md bg-amberline px-4 py-2 text-sm font-semibold text-graphite transition hover:bg-[#e6bb72] focus:outline-none focus:ring-2 focus:ring-amberline focus:ring-offset-2 focus:ring-offset-panel disabled:cursor-not-allowed disabled:opacity-50"
                  onclick={downloadActiveAlbum}
                  disabled={downloading}
                >
                  {downloading ? '下载中...' : '下载整张专辑'}
                </button>
                {#if downloading}
                  <button
                    class="rounded-md border border-red-300/60 px-4 py-2 text-sm font-semibold text-red-200 transition hover:bg-red-300 hover:text-graphite focus:outline-none focus:ring-2 focus:ring-red-300 focus:ring-offset-2 focus:ring-offset-panel"
                    onclick={cancelDownload}
                  >
                    取消下载
                  </button>
                {/if}
                <span class="text-sm text-zinc-500">{activeAlbum.songs.length} tracks</span>
              </div>

              {#if downloading || currentTrack || downloadIssues.length > 0}
                <div class="mt-5 rounded-lg border border-white/10 bg-black/20 p-4">
                  <div class="flex items-center justify-between gap-3 text-xs text-zinc-400">
                    <span class="truncate">{currentTrack ? `当前曲目：${currentTrack}` : '等待曲目开始...'}</span>
                    <span>{downloadPercent}%</span>
                  </div>
                  <div class="mt-2 h-2 overflow-hidden rounded-full bg-white/10">
                    <div class="h-full rounded-full bg-amberline transition-all" style={`width: ${downloadPercent}%`}></div>
                  </div>
                  {#if downloadIssues.length > 0}
                    <p class="mt-3 text-xs text-red-300">最近问题：{downloadIssues[downloadIssues.length - 1].message}</p>
                  {/if}
                </div>
              {/if}

              <ol class="mt-6 divide-y divide-white/10 overflow-hidden rounded-lg border border-white/10">
                {#each activeAlbum.songs as song, index}
                  <li class="grid grid-cols-[2.5rem_1fr] gap-3 bg-white/[0.02] px-3 py-3 text-sm">
                    <span class="font-mono text-xs text-zinc-500">{String(index + 1).padStart(2, '0')}</span>
                    <span class="min-w-0">
                      <span class="block truncate text-zinc-100">{song.name}</span>
                      <span class="mt-1 block truncate text-xs text-zinc-500">{visibleArtists(song.artistes)}</span>
                    </span>
                  </li>
                {/each}
              </ol>
            </div>
          </div>
        {:else}
          <div class="flex min-h-[26rem] items-center justify-center rounded-xl border border-dashed border-white/15 p-8 text-center text-sm text-zinc-400">
            选择一张专辑查看详情。
          </div>
        {/if}

        <div class="mt-5 rounded-lg border border-white/10 bg-black/20 px-4 py-3" role="status">
          <p class="text-sm text-zinc-300">{status}</p>
          {#if error}
            <p class="mt-2 text-sm text-red-300">{error}</p>
          {/if}
        </div>
      </section>
    </div>
  </section>
</main>
