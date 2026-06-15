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

  const browserApiBase = '/msr-api';

  let albums = $state<AlbumBrief[]>([]);
  let activeAlbum = $state<AlbumDetail | null>(null);
  let selectedCid = $state<string | null>(null);
  let search = $state('');
  let loadingAlbums = $state(false);
  let loadingDetail = $state(false);
  let downloading = $state(false);
  let status = $state('正在准备专辑列表。');
  let error = $state<string | null>(null);
  let currentTrack = $state<string | null>(null);
  let downloadPercent = $state(0);
  let downloadIssues = $state<DownloadIssue[]>([]);
  let coverDataUrls = $state<Map<string, string>>(new Map());
  let loadingCoverUrls = $state<Set<string>>(new Set());
  let failedCoverUrls = $state<Set<string>>(new Set());

  let runningInTauri = $state(false);
  let activeCoverUrl = $derived(activeAlbum ? activeAlbum.coverUrl : '');
  let filteredAlbums = $derived(
    albums.filter((album) => {
      const needle = search.trim().toLowerCase();
      if (!needle) return true;
      return [album.cid, album.name, visibleArtists(album.artistes)]
        .join(' ')
        .toLowerCase()
        .includes(needle);
    }),
  );

  const visibleArtists = (artists?: string[]) => artists?.filter(Boolean).join(' / ') || '塞壬唱片-MSR';
  const coverImageSrc = (url: string) => {
    if (coverDataUrls.has(url)) return coverDataUrls.get(url) || url;
    return runningInTauri ? url : url.replace('https://web.hycdn.cn', '/msr-img');
  };

  async function apiFetch<T>(path: string): Promise<T> {
    const response = await fetch(`${browserApiBase}/${path}`);
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    const payload = (await response.json()) as { code: number; msg: string; data: T };
    if (payload.code !== 0) throw new Error(payload.msg || `API returned code ${payload.code}`);
    return payload.data;
  }

  async function loadAlbums() {
    if (loadingAlbums) return;
    loadingAlbums = true;
    error = null;
    status = '正在同步塞壬唱片专辑列表...';
    try {
      albums = runningInTauri ? await invoke<AlbumBrief[]>('list_albums') : await apiFetch<AlbumBrief[]>('albums');
      status = `已加载 ${albums.length} 张专辑。`;
      if (!selectedCid && albums[0]) await selectAlbum(albums[0].cid);
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
      activeAlbum = runningInTauri
        ? await invoke<AlbumDetail>('get_album_detail', { cid })
        : await apiFetch<AlbumDetail>(`album/${cid}/detail`);
      status = `已选择 ${activeAlbum.name}，共 ${activeAlbum.songs.length} 首。`;
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
    if (!runningInTauri) {
      error = '下载功能需要在 Tauri 桌面端中使用。';
      return;
    }

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
      status = '下载启动失败。';
      downloading = false;
    }
  }

  async function cancelDownload() {
    if (!downloading || !runningInTauri) return;
    try {
      await invoke<void>('cancel_download');
      status = '正在取消下载...';
    } catch (err) {
      error = String(err);
    }
  }

  async function loadCoverDataUrl(url: string) {
    if (!url || coverDataUrls.has(url) || loadingCoverUrls.has(url) || failedCoverUrls.has(url)) return;
    if (!runningInTauri) {
      markCoverFailed(url);
      return;
    }

    loadingCoverUrls = new Set([...loadingCoverUrls, url]);
    try {
      const dataUrl = await invoke<string>('fetch_cover_data_url', { url });
      coverDataUrls = new Map([...coverDataUrls, [url, dataUrl]]);
    } catch {
      markCoverFailed(url);
    } finally {
      const nextLoading = new Set(loadingCoverUrls);
      nextLoading.delete(url);
      loadingCoverUrls = nextLoading;
    }
  }

  function markCoverFailed(url: string) {
    failedCoverUrls = new Set([...failedCoverUrls, url]);
  }

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
        if (event.status === 'failed') error = `${event.name} 下载失败。`;
        break;
      case 'issue':
        downloadIssues = [...downloadIssues, event.issue];
        break;
      case 'albumFinished': {
        const done = event.report.tracks.filter((track) => track.status === 'done').length;
        const failed = event.report.tracks.filter((track) => track.status === 'failed').length;
        downloading = false;
        downloadPercent = 100;
        status = `${event.report.albumName} 下载完成：${done}/${event.report.totalTracks} 首，失败 ${failed} 首。`;
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
    runningInTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
    void loadAlbums();

    if (!runningInTauri) return;
    const unlisten = listen<DownloadEvent>('download-event', (event) => applyDownloadEvent(event.payload));
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  });

  $effect(() => {
    if (loadingAlbums || loadingDetail || filteredAlbums.length === 0) return;
    if (selectedCid && filteredAlbums.some((album) => album.cid === selectedCid)) return;
    void selectAlbum(filteredAlbums[0].cid);
  });
</script>

<main class="msr-shell h-screen overflow-hidden text-[#d4d8dd]">
  <div class="relative z-10 mx-auto flex h-screen w-full max-w-[92rem] flex-col px-6 py-4">
    <header class="flex flex-col gap-4 border-b border-[#d4d8dd]/15 pb-4 lg:flex-row lg:items-center lg:justify-between">
      <div>
        <p class="text-[0.68rem] font-semibold uppercase tracking-[0.42em] text-[#8d949e]">Monster Siren Records</p>
        <h1 class="mt-1 text-2xl font-semibold tracking-[-0.04em] text-[#f0f2f5]">MSR Downloader</h1>
      </div>

      <div class="flex flex-col gap-3 sm:flex-row sm:items-center">
        <label class="msr-input-frame flex h-10 min-w-0 items-center bg-[#0d0f12] px-3 sm:w-80">
          <span class="sr-only">搜索专辑</span>
          <input
            class="w-full bg-transparent text-sm text-[#f0f2f5] outline-none placeholder:text-[#707782]"
            type="search"
            bind:value={search}
            placeholder="搜索专辑 / 艺术家 / CID"
          />
        </label>
        <button
          class="border border-[#d4d8dd]/35 px-4 py-2 text-sm font-semibold text-[#f0f2f5] transition hover:border-[#f0f2f5] hover:bg-[#f0f2f5] hover:text-[#09090b] disabled:opacity-50"
          onclick={loadAlbums}
          disabled={loadingAlbums}
        >
          {loadingAlbums ? '同步中' : '刷新'}
        </button>
      </div>
    </header>

    <section class="grid min-h-0 flex-1 gap-5 py-4 lg:grid-cols-[20rem_minmax(0,1fr)_20rem]">
      <aside class="flex min-h-0 flex-col border border-[#d4d8dd]/14 bg-[#101216]/90">
        <div class="flex items-center justify-between border-b border-[#d4d8dd]/12 px-4 py-3">
          <h2 class="text-sm font-semibold text-[#f0f2f5]">专辑列表</h2>
          <span class="text-xs text-[#8d949e]">{filteredAlbums.length}/{albums.length}</span>
        </div>

        {#if loadingAlbums && albums.length === 0}
          <div class="space-y-3 p-4" aria-busy="true" aria-label="正在加载专辑">
            {#each Array.from({ length: 8 }) as _}
              <div class="h-16 bg-[#d4d8dd]/8"></div>
            {/each}
          </div>
        {:else if filteredAlbums.length === 0}
          <div class="p-6 text-sm leading-6 text-[#8d949e]">
            {albums.length === 0 ? '无法加载专辑。请检查网络或点击刷新重试。' : '没有匹配的专辑。'}
          </div>
        {:else}
          <div class="msr-scrollbar min-h-0 flex-1 overflow-y-auto p-2">
            {#each filteredAlbums as album}
              <button
                class="grid w-full grid-cols-[3.4rem_1fr] gap-3 border p-2 text-left transition focus:outline-none focus:ring-1 focus:ring-[#f0f2f5] {selectedCid === album.cid ? 'border-[#f0f2f5] bg-[#d4d8dd]/10' : 'border-transparent hover:border-[#d4d8dd]/20 hover:bg-[#d4d8dd]/6'}"
                onclick={() => selectAlbum(album.cid)}
              >
                <span class="relative aspect-square overflow-hidden bg-[#15181d]">
                  {#if album.coverUrl && !failedCoverUrls.has(album.coverUrl)}
                    <img
                      class="h-full w-full object-cover"
                      src={coverImageSrc(album.coverUrl)}
                      alt="{album.name} cover"
                      loading="lazy"
                      onerror={() => void loadCoverDataUrl(album.coverUrl)}
                    />
                  {:else}
                    <span class="grid h-full w-full place-items-center text-[0.62rem] text-[#8d949e]">MSR</span>
                  {/if}
                </span>
                <span class="min-w-0 self-center">
                  <span class="block truncate text-sm font-medium text-[#f0f2f5]">{album.name}</span>
                  <span class="mt-1 block truncate text-xs text-[#8d949e]">{visibleArtists(album.artistes)}</span>
                </span>
              </button>
            {/each}
          </div>
        {/if}
      </aside>

      <section class="min-h-0 min-w-0 overflow-hidden border border-[#d4d8dd]/14 bg-[#101216]/90">
        {#if loadingDetail}
          <div class="grid gap-6 p-5 md:grid-cols-[16rem_1fr]" aria-busy="true" aria-label="正在加载专辑详情">
            <div class="aspect-square bg-[#d4d8dd]/8"></div>
            <div class="space-y-4">
              <div class="h-8 w-2/3 bg-[#d4d8dd]/8"></div>
              <div class="h-4 w-full bg-[#d4d8dd]/8"></div>
              <div class="h-4 w-5/6 bg-[#d4d8dd]/8"></div>
            </div>
          </div>
        {:else if activeAlbum}
          <div class="grid h-full min-h-0 gap-6 p-5 xl:grid-cols-[16rem_1fr]">
            <div>
              <div class="msr-cover-shadow aspect-square overflow-hidden bg-[#15181d]">
                {#if activeCoverUrl && !failedCoverUrls.has(activeCoverUrl)}
                  <img
                    class="h-full w-full object-cover"
                    src={coverImageSrc(activeCoverUrl)}
                    alt="{activeAlbum.name} cover"
                    onerror={() => void loadCoverDataUrl(activeCoverUrl)}
                  />
                {:else}
                  <div class="grid h-full w-full place-items-center p-6 text-center text-3xl font-semibold tracking-[-0.08em] text-[#d4d8dd]/75">
                    {activeAlbum.name}
                  </div>
                {/if}
              </div>

              <div class="mt-4 grid grid-cols-2 gap-2 text-xs text-[#8d949e]">
                <div class="border border-[#d4d8dd]/12 p-3">
                  <p class="text-lg font-semibold text-[#f0f2f5]">{activeAlbum.songs.length}</p>
                  <p>曲目</p>
                </div>
                <div class="border border-[#d4d8dd]/12 p-3">
                  <p class="text-lg font-semibold text-[#f0f2f5]">{activeAlbum.cid}</p>
                  <p>CID</p>
                </div>
              </div>
            </div>

            <div class="flex min-h-0 min-w-0 flex-col">
              <p class="text-xs uppercase tracking-[0.28em] text-[#8d949e]">{activeAlbum.belong || 'Monster Siren Records'}</p>
              <h2 class="mt-2 text-3xl font-semibold tracking-[-0.05em] text-[#f0f2f5]">{activeAlbum.name}</h2>
              <p class="mt-4 max-h-32 overflow-y-auto whitespace-pre-line pr-2 text-sm leading-7 text-[#a2a9ae] msr-scrollbar">
                {activeAlbum.intro || '暂无专辑介绍。'}
              </p>

              <div class="mt-5 flex flex-wrap items-center gap-3">
                <button
                  class="border border-[#f0f2f5] bg-[#f0f2f5] px-5 py-2.5 text-sm font-semibold text-[#09090b] transition hover:bg-transparent hover:text-[#f0f2f5] disabled:opacity-50"
                  onclick={downloadActiveAlbum}
                  disabled={downloading || !activeAlbum}
                  title={runningInTauri ? '下载整张专辑' : '下载功能需要桌面端'}
                >
                  {downloading ? '下载中' : '下载整张专辑'}
                </button>
                {#if downloading}
                  <button
                    class="border border-[#d88b8b]/70 px-5 py-2.5 text-sm font-semibold text-[#efb0b0] transition hover:bg-[#d88b8b] hover:text-[#09090b]"
                    onclick={cancelDownload}
                  >
                    取消下载
                  </button>
                {/if}
                {#if !runningInTauri}
                  <span class="text-xs text-[#8d949e]">浏览器预览仅支持浏览，下载请启动桌面端。</span>
                {/if}
              </div>

              <div class="mt-6 overflow-hidden border border-[#d4d8dd]/12">
                <div class="flex items-center justify-between border-b border-[#d4d8dd]/12 px-4 py-3">
                  <h3 class="text-sm font-semibold text-[#f0f2f5]">曲目</h3>
                  <span class="text-xs text-[#8d949e]">{activeAlbum.songs.length} tracks</span>
                </div>
                <ol class="msr-scrollbar max-h-[18rem] overflow-y-auto">
                  {#each activeAlbum.songs as song, index}
                    <li class="grid grid-cols-[3rem_1fr] gap-3 border-b border-[#d4d8dd]/8 px-4 py-3 text-sm last:border-b-0">
                      <span class="font-mono text-xs text-[#707782]">{String(index + 1).padStart(2, '0')}</span>
                      <span class="min-w-0">
                        <span class="block truncate text-[#d4d8dd]">{song.name}</span>
                        <span class="mt-1 block truncate text-xs text-[#707782]">{visibleArtists(song.artistes)}</span>
                      </span>
                    </li>
                  {/each}
                </ol>
              </div>
            </div>
          </div>
        {:else}
          <div class="grid min-h-[32rem] place-items-center p-8 text-center text-sm text-[#8d949e]">
            <div>
              <p class="text-lg font-semibold text-[#d4d8dd]">选择一张专辑</p>
              <p class="mt-2">左侧列表加载后，点击专辑查看曲目与下载操作。</p>
            </div>
          </div>
        {/if}
      </section>

      <aside class="flex min-h-0 flex-col border border-[#d4d8dd]/14 bg-[#101216]/90">
        <div class="border-b border-[#d4d8dd]/12 px-4 py-3">
          <h2 class="text-sm font-semibold text-[#f0f2f5]">下载状态</h2>
        </div>
        <div class="msr-scrollbar min-h-0 flex-1 space-y-4 overflow-y-auto p-4">
          <div>
            <div class="flex items-center justify-between text-xs text-[#8d949e]">
              <span>{currentTrack ? '当前曲目' : '状态'}</span>
              <span>{downloadPercent}%</span>
            </div>
            <p class="mt-2 min-h-10 text-sm leading-5 text-[#d4d8dd]">{currentTrack || status}</p>
            <div class="mt-3 h-2 bg-[#d4d8dd]/10">
              <div class="h-full bg-[#f0f2f5] transition-all" style={`width: ${downloadPercent}%`}></div>
            </div>
          </div>

          {#if downloadIssues.length > 0}
            <div class="border border-[#d88b8b]/30 bg-[#d88b8b]/8 p-3 text-sm text-[#efb0b0]">
              最近问题：{downloadIssues[downloadIssues.length - 1].message}
            </div>
          {/if}

          {#if error}
            <div class="border border-[#d88b8b]/40 bg-[#d88b8b]/10 p-3 text-sm leading-6 text-[#efb0b0]">
              {error}
            </div>
          {/if}

          <div class="border border-[#d4d8dd]/12 p-3 text-xs leading-6 text-[#8d949e]">
            <p>打开后自动同步专辑列表。</p>
            <p>搜索支持专辑名、艺术家和 CID。</p>
            <p>封面直连失败时会在桌面端自动走后端代理。</p>
          </div>
        </div>
      </aside>
    </section>

    <footer class="border-t border-[#d4d8dd]/15 py-3 text-xs text-[#8d949e]" role="status" aria-live="polite">
      {status}
    </footer>
  </div>
</main>
