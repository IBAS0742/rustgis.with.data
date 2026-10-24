/* RustGIS Preview console. Plain JavaScript on purpose: the map stays in the RustGIS WASM app,
   while this page owns server-specific state (immutable versions, source roots, DB passwords). */
(() => {
  'use strict';

  const $ = (id) => document.getElementById(id);
  const enc = encodeURIComponent;
  // A non-COG TIFF is downloaded whole by the map's URL import and embedded in the project.
  // Keep that path to small files; larger rasters must be added as COGs from the map itself.
  const MAX_AUTO_TIFF_BYTES = 64 * 1024 * 1024;
  const state = {
    health: null,
    webReady: false,
    mapReady: false,
    commandSeq: 0,
    waiters: new Map(),
    pendingCommands: [],
    pendingProject: null,
    projectText: null,
    loadedText: null,
    dirty: false,
    current: null,
    availability: null,
    projects: [],
    sources: [],
    browse: null,
    dbNames: null,
    catalog: null,
    selectedDb: null,
  };

  function esc(value) {
    return String(value ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  }
  function badge(status) {
    const cls = status === 'available' ? 'ok' : status === 'unavailable' ? 'bad' : 'warn';
    return `<span class="badge ${cls}">${esc(status)}</span>`;
  }
  function toast(message, error) {
    const el = $('toast');
    el.textContent = String(message ?? '');
    el.className = error ? 'error show' : 'show';
    clearTimeout(toast.timer);
    toast.timer = setTimeout(() => { el.className = error ? 'error' : ''; }, 5200);
  }
  function errorText(e) {
    return e && e.message ? e.message : String(e);
  }
  function isTiffName(name) {
    return /\.(tif|tiff)$/i.test(String(name || ''));
  }
  function formatBytes(bytes) {
    const n = Number(bytes || 0);
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
    return `${(n / (1024 * 1024)).toFixed(1)} MiB`;
  }

  async function api(path, options = {}) {
    const opts = { ...options, headers: { ...(options.headers || {}) } };
    if (opts.body && typeof opts.body !== 'string') {
      opts.headers['Content-Type'] = 'application/json';
      opts.body = JSON.stringify(opts.body);
    }
    const response = await fetch(path, opts);
    const text = await response.text();
    let data = null;
    if (text) {
      try { data = JSON.parse(text); } catch { data = { error: { message: text } }; }
    }
    if (!response.ok) {
      const message = data && data.error && data.error.message ? data.error.message : `HTTP ${response.status}`;
      throw new Error(message);
    }
    return data;
  }

  // ---- map iframe ---------------------------------------------------------
  const mapFrame = $('map');
  function postMap(message) {
    if (mapFrame.contentWindow) mapFrame.contentWindow.postMessage(message, location.origin);
  }
  function sendProject(text) {
    state.projectText = text;
    state.pendingProject = text;
    if (state.mapReady) {
      postMap({ rustgis: 'project', text });
      state.pendingProject = null;
    }
  }
  function runCommand(command, params = {}) {
    return new Promise((resolve, reject) => {
      if (!state.mapReady) {
        state.pendingCommands.push({ command, params, resolve, reject });
        return;
      }
      const id = ++state.commandSeq;
      const timer = setTimeout(() => {
        state.waiters.delete(id);
        reject(new Error(`map command ${command} timed out`));
      }, 120000);
      state.waiters.set(id, {
        resolve: (value) => { clearTimeout(timer); resolve(value); },
        reject: (e) => { clearTimeout(timer); reject(e); },
      });
      postMap({ rustgis: 'run', id, command, params });
    });
  }
  function mapReady() {
    state.mapReady = true;
    if (state.pendingProject !== null) {
      postMap({ rustgis: 'project', text: state.pendingProject });
      state.pendingProject = null;
    }
    const pending = state.pendingCommands.splice(0);
    for (const p of pending) {
      runCommand(p.command, p.params).then(p.resolve, p.reject);
    }
  }
  window.addEventListener('message', (event) => {
    if (event.origin !== location.origin || event.source !== mapFrame.contentWindow) return;
    const m = event.data;
    if (!m || typeof m !== 'object') return;
    if (m.rustgis === 'ready') {
      mapReady();
    } else if (m.rustgis === 'project') {
      if (typeof m.text !== 'string') return;
      state.projectText = m.text;
      state.dirty = state.loadedText !== null && m.text !== state.loadedText;
      renderMapBar();
    } else if (m.rustgis === 'result') {
      const waiter = state.waiters.get(m.id);
      if (!waiter) return;
      state.waiters.delete(m.id);
      if (m.ok) waiter.resolve(m.value);
      else waiter.reject(new Error(m.error || 'map command failed'));
    }
  });

  // ---- health and project state ------------------------------------------
  async function loadHealth() {
    try {
      state.health = await api('/api/health');
      state.webReady = !!(state.health && state.health.web_ready);
      $('health-dot').className = 'dot ok';
      $('health-text').textContent = state.webReady ? 'API online · map ready' : 'API online · map build missing';
      $('map-overlay').style.display = state.webReady ? 'none' : 'flex';
    } catch (e) {
      $('health-dot').className = 'dot bad';
      $('health-text').textContent = 'API unavailable';
      toast(`Cannot reach the server: ${errorText(e)}`, true);
    }
  }

  function renderMapBar() {
    const c = state.current;
    $('map-project').textContent = c ? c.project.name : 'No project open';
    $('map-mode').textContent = !c ? 'blank map' : c.workingId ? 'working copy' : 'read-only version';
    $('map-mode').className = `badge ${!c ? '' : c.workingId ? 'ok' : 'warn'}`;
    $('map-dirty').textContent = state.dirty ? 'unsaved changes' : (c ? 'saved' : '');
    $('copy-version').disabled = !c || !!c.workingId;
    $('save-version').disabled = !c || !c.workingId || !state.webReady;
  }

  function renderProjectDetail() {
    const c = state.current;
    const el = $('project-detail');
    if (!c) { el.style.display = 'none'; el.innerHTML = ''; return; }
    el.style.display = 'block';
    const base = c.workingId ? `<div class="meta">working copy <span class="mono">${esc(c.workingId)}</span>${c.baseVersion ? ` · base ${esc(c.baseVersion)}` : ''}</div>` : `<div class="meta">version <span class="mono">${esc(c.versionId || '')}</span> · immutable</div>`;
    el.innerHTML = `<div class="card"><h3>${esc(c.project.name)}</h3>
      <div class="meta">project <span class="mono">${esc(c.project.id)}</span></div>${base}
      <div class="row" style="margin-top:7px">${c.workingId ? `<button data-act="open-working" data-project="${esc(c.project.id)}" data-working="${esc(c.workingId)}" type="button">Reload working copy</button>` : ''}</div></div>`;
  }

  function availabilityHtml(report) {
    if (!report) return '<p class="meta">No availability report.</p>';
    const layers = report.layers || [];
    if (!layers.length) return '<p class="meta">This project has no layers.</p>';
    return layers.map((l) => {
      const name = l.name || `Layer ${l.layer_id}`;
      return `<div class="layerstatus"><span><strong>${esc(name)}</strong> <span class="muted">· ${esc(l.map_name)}</span></span>${badge(l.status)}<span class="reason">${esc(l.reason || '')}${l.source_id ? ` · source ${esc(l.source_id)}` : ''}</span></div>`;
    }).join('');
  }
  function renderAvailability() {
    const panel = $('availability');
    if (!state.current) { panel.style.display = 'none'; return; }
    panel.style.display = 'block';
    $('availability-list').innerHTML = availabilityHtml(state.availability);
  }

  async function refreshProjects() {
    const data = await api('/api/projects');
    state.projects = data.projects || [];
    renderProjects();
  }
  function renderProjects() {
    const el = $('project-list');
    if (!state.projects.length) {
      el.innerHTML = '<p class="meta">No saved projects yet. Create one above; every project starts with an immutable V1.</p>';
      return;
    }
    el.innerHTML = state.projects.map((p) => {
      const versions = (p.versions || []).slice().reverse().map((v) =>
        `<div class="version"><div class="row"><span class="mono">${esc(v.id)}</span>${state.current && state.current.versionId === v.id ? '<span class="badge ok">open</span>' : ''}</div>
         <div class="meta">${new Date(v.created_at_ms || 0).toLocaleString()}${v.parent ? ` · parent ${esc(v.parent)}` : ''} · ${v.size || 0} bytes</div>
         <div class="row"><button data-act="open-version" data-project="${esc(p.id)}" data-version="${esc(v.id)}" type="button">Open</button>
         <button data-act="copy-version" data-project="${esc(p.id)}" data-version="${esc(v.id)}" type="button">Copy to working</button></div></div>`).join('');
      const workings = (p.working_copies || []).map((w) =>
        `<div class="version"><div class="row"><span class="mono">${esc(w.id)}</span><span class="badge">working</span></div>
         <div class="meta">base ${esc(w.base_version || '—')} · ${new Date(w.updated_at_ms || 0).toLocaleString()}</div>
         <div class="row"><button data-act="open-working" data-project="${esc(p.id)}" data-working="${esc(w.id)}" type="button">Open working copy</button></div></div>`).join('');
      return `<div class="card"><h3>${esc(p.name)}</h3><div class="meta">project <span class="mono">${esc(p.id)}</span> · ${(p.versions || []).length} version(s)</div>${versions}${workings}</div>`;
    }).join('');
  }

  function confirmDiscard() {
    return !state.dirty || window.confirm('The current map has unsaved changes. Discard them and continue?');
  }

  async function openVersion(projectId, versionId) {
    if (!confirmDiscard()) return;
    try {
      const data = await api(`/api/projects/${enc(projectId)}/versions/${enc(versionId)}/prepared`);
      state.current = { project: data.project, versionId, workingId: null, readOnly: true };
      state.availability = data.availability;
      state.loadedText = data.content;
      state.dirty = false;
      sendProject(data.content);
      renderProjectDetail(); renderAvailability(); renderMapBar(); renderProjects();
      toast(`Opened ${projectId} / ${versionId} read-only`);
    } catch (e) { toast(`Could not open version: ${errorText(e)}`, true); }
  }
  async function openWorking(projectId, workingId) {
    if (!confirmDiscard()) return;
    try {
      const data = await api(`/api/projects/${enc(projectId)}/working/${enc(workingId)}/prepared`);
      state.current = { project: data.project, versionId: null, workingId, baseVersion: data.base_version || null, readOnly: false };
      state.availability = data.availability;
      state.loadedText = data.content;
      state.dirty = false;
      sendProject(data.content);
      renderProjectDetail(); renderAvailability(); renderMapBar(); renderProjects();
      toast(`Opened working copy ${workingId}; saving creates a new immutable version`);
    } catch (e) { toast(`Could not open working copy: ${errorText(e)}`, true); }
  }

  async function createProject() {
    const name = $('new-project-name').value.trim();
    if (!name) { toast('Enter a project name first', true); return; }
    try {
      const created = await api('/api/projects', { method: 'POST', body: { name, content: null } });
      $('new-project-name').value = '';
      await refreshProjects();
      await openVersion(created.project.id, created.version.id);
      toast(`Created ${name} with immutable V1`);
    } catch (e) { toast(`Could not create project: ${errorText(e)}`, true); }
  }

  async function copyCurrentVersion() {
    const c = state.current;
    if (!c || !c.versionId) { toast('Open a read-only version first', true); return; }
    if (!confirmDiscard()) return;
    try {
      const w = await api(`/api/projects/${enc(c.project.id)}/copies`, { method: 'POST', body: { versionId: c.versionId } });
      state.dirty = false;
      await refreshProjects();
      await openWorking(c.project.id, w.id);
      toast(`Created working copy ${w.id}. Save it to publish a new version.`);
    } catch (e) { toast(`Could not copy version: ${errorText(e)}`, true); }
  }

  async function saveWorkingCopy() {
    const c = state.current;
    if (!c || !c.workingId) { toast('Open a working copy before saving', true); return; }
    if (!state.projectText) { toast('The map has not delivered a project yet', true); return; }
    try {
      // The map is showing the derived, availability-prepared copy. Store the original
      // visibility for layers the check hid, so the saved version can recover when the data
      // returns; opening it will still hide them while they are unavailable.
      const content = restoreUnavailableVisibility(state.projectText, state.availability);
      const version = await api(`/api/projects/${enc(c.project.id)}/working/${enc(c.workingId)}/versions`, {
        method: 'POST', body: { content },
      });
      c.versionId = version.id;
      state.loadedText = state.projectText;
      state.dirty = false;
      try { c.project = await api(`/api/projects/${enc(c.project.id)}`); } catch { /* best effort */ }
      try {
        const prepared = await api(`/api/projects/${enc(c.project.id)}/versions/${enc(version.id)}/prepared`);
        state.availability = prepared.availability;
      } catch { /* the published version is still saved */ }
      await refreshProjects();
      renderProjectDetail(); renderAvailability(); renderMapBar();
      toast(`Published immutable version ${version.id}`);
    } catch (e) { toast(`Could not save a new version: ${errorText(e)}`, true); }
  }

  async function recheckAvailability() {
    const c = state.current;
    if (!c || !state.projectText) { toast('Open a project first', true); return; }
    try {
      const wasDirty = state.dirty;
      let data;
      if (wasDirty) {
        // Keep the user's in-progress edits, then only un-hide layers that were hidden by the
        // previous availability check and have now recovered. User-hidden layers stay hidden.
        data = await api(`/api/projects/${enc(c.project.id)}/availability`, { method: 'POST', body: { content: state.projectText } });
        data.content = restoreRecoveredLayers(data.content, state.availability, data.availability);
      } else {
        // Re-read the stored project so original visibility is restored, not the previously
        // prepared (hidden) copy that the map is currently showing.
        const path = c.workingId
          ? `/api/projects/${enc(c.project.id)}/working/${enc(c.workingId)}/prepared`
          : `/api/projects/${enc(c.project.id)}/versions/${enc(c.versionId)}/prepared`;
        data = await api(path);
      }
      state.availability = data.availability;
      if (!wasDirty) state.loadedText = data.content;
      state.dirty = wasDirty;
      sendProject(data.content);
      renderAvailability(); renderMapBar();
      toast(data.availability.unavailable ? `${data.availability.unavailable} layer(s) unavailable and hidden` : 'All checked references are available');
    } catch (e) { toast(`Could not recheck layers: ${errorText(e)}`, true); }
  }

  function restoreUnavailableVisibility(content, report) {
    if (!report || !Array.isArray(report.layers)) return content;
    let value;
    try { value = JSON.parse(content); } catch { return content; }
    for (const entry of report.layers) {
      const map = (value.maps || [])[entry.map_index];
      if (!map) continue;
      if (entry.tile_index !== undefined && entry.tile_index !== null) {
        const tile = (Array.isArray(map.tiles) ? map.tiles : [])[entry.tile_index];
        if (!tile) continue;
        // The marker is a derived-view detail. Never persist it, even when the tile was
        // originally hidden and must stay hidden.
        delete tile.x_rustgis_availability;
        if (entry.status === 'unavailable' && entry.original_visible) tile.visible = true;
        continue;
      }
      const layer = (Array.isArray(map.layers) ? map.layers : []).find((item) => item && item.id === entry.layer_id);
      if (!layer) continue;
      delete layer.x_rustgis_availability;
      if (entry.status === 'unavailable' && entry.original_visible) layer.visible = true;
    }
    return JSON.stringify(value);
  }

  function availabilityKey(entry) {
    return entry && entry.tile_index !== undefined && entry.tile_index !== null
      ? `tile:${entry.map_index}:${entry.tile_index}`
      : `layer:${entry.map_index}:${entry.layer_id}`;
  }
  function restoreRecoveredLayers(content, previous, next) {
    if (!previous || !next) return content;
    const hidden = new Set();
    for (const layer of previous.layers || []) {
      if (layer.status === 'unavailable' && layer.original_visible) hidden.add(availabilityKey(layer));
    }
    if (!hidden.size) return content;
    const available = new Set();
    for (const layer of next.layers || []) {
      if (layer.status === 'available') available.add(availabilityKey(layer));
    }
    let value;
    try { value = JSON.parse(content); } catch { return content; }
    for (let mi = 0; mi < (value.maps || []).length; mi++) {
      const map = value.maps[mi];
      for (const layer of (map.layers || [])) {
        const key = `layer:${mi}:${layer.id}`;
        if (!hidden.has(key)) continue;
        const report = (next.layers || []).find((entry) => entry.map_index === mi && entry.layer_id === layer.id && (entry.tile_index === undefined || entry.tile_index === null));
        if (!report) continue;
        // Remember the pre-check visibility across a dirty recheck, even if the file is still
        // missing. The map stays hidden, but saving must not turn the original layer into a
        // permanently hidden one.
        report.original_visible = true;
        if (!available.has(key)) continue;
        layer.visible = true;
        delete layer.x_rustgis_availability;
      }
      for (let ti = 0; ti < (map.tiles || []).length; ti++) {
        const key = `tile:${mi}:${ti}`;
        if (!hidden.has(key)) continue;
        const report = (next.layers || []).find((entry) => entry.map_index === mi && entry.tile_index === ti);
        if (!report) continue;
        report.original_visible = true;
        if (!available.has(key)) continue;
        map.tiles[ti].visible = true;
        delete map.tiles[ti].x_rustgis_availability;
      }
    }
    return JSON.stringify(value);
  }

  // ---- sources ------------------------------------------------------------
  async function refreshSources() {
    const data = await api('/api/sources/folders');
    state.sources = data.folders || [];
    renderSources();
  }
  function renderSources() {
    const el = $('source-list');
    if (!state.sources.length) {
      el.innerHTML = '<p class="meta">No source folders configured. Add a directory on the server above.</p>';
      return;
    }
    el.innerHTML = state.sources.map((s) => {
      const status = s.scan_status === 'ready' ? 'ok' : s.scan_status === 'unavailable' || s.scan_status === 'error' ? 'bad' : 'warn';
      return `<div class="card"><h3>${esc(s.display_name)}</h3><div class="mono">${esc(s.root_path)}</div>
        <div class="row" style="margin-top:5px"><span class="badge ${status}">${esc(s.scan_status)}</span><span class="badge">${s.enabled ? 'enabled' : 'disabled'}</span>
        <button data-act="browse-source" data-id="${esc(s.id)}" type="button">Browse</button>
        <button data-act="scan-source" data-id="${esc(s.id)}" type="button">Rescan</button>
        <button data-act="toggle-source" data-id="${esc(s.id)}" data-enabled="${s.enabled ? '1' : '0'}" type="button">${s.enabled ? 'Disable' : 'Enable'}</button>
        <button class="danger" data-act="remove-source" data-id="${esc(s.id)}" type="button">Remove</button></div>
        ${s.last_error ? `<div class="meta" style="color:var(--danger)">${esc(s.last_error)}</div>` : ''}</div>`;
    }).join('');
  }

  async function addSource() {
    const path = $('source-path').value.trim();
    const name = $('source-name').value.trim();
    if (!path) { toast('Enter an existing server directory', true); return; }
    try {
      await api('/api/sources/folders', { method: 'POST', body: { path, displayName: name || null } });
      $('source-path').value = ''; $('source-name').value = '';
      await refreshSources();
      toast('Source folder added. New files are browseable but are not added to any project automatically.');
    } catch (e) { toast(`Could not add folder: ${errorText(e)}`, true); }
  }

  async function sourceAction(button) {
    const act = button.dataset.act;
    const id = button.dataset.id;
    try {
      if (act === 'browse-source') {
        await loadBrowse(id, '', 0);
      } else if (act === 'scan-source') {
        const summary = await api(`/api/sources/folders/${enc(id)}/scan`, { method: 'POST' });
        await refreshSources();
        toast(summary.error ? `Rescan reported: ${summary.error}` : `Rescan: ${summary.files} files, ${summary.directories} directories`, !!summary.error);
      } else if (act === 'toggle-source') {
        await api(`/api/sources/folders/${enc(id)}`, { method: 'PATCH', body: { enabled: button.dataset.enabled !== '1' } });
        await refreshSources();
      } else if (act === 'remove-source') {
        if (!window.confirm('Remove this source folder? The files on disk are not deleted.')) return;
        await api(`/api/sources/folders/${enc(id)}`, { method: 'DELETE' });
        state.browse = null;
        $('source-browser').style.display = 'none';
        await refreshSources();
        toast('Source removed');
      }
    } catch (e) { toast(`Source operation failed: ${errorText(e)}`, true); }
  }

  async function loadBrowse(sourceId, path, page) {
    try {
      const data = await api(`/api/sources/folders/${enc(sourceId)}/items?path=${enc(path)}&page=${page}&page_size=50`);
      state.browse = data;
      renderBrowse();
    } catch (e) { toast(`Could not browse: ${errorText(e)}`, true); }
  }
  function renderBrowse() {
    const b = state.browse;
    const panel = $('source-browser');
    if (!b) { panel.style.display = 'none'; return; }
    panel.style.display = 'block';
    const source = state.sources.find((s) => s.id === b.source_id);
    const parts = b.path ? b.path.split('/').filter(Boolean) : [];
    let cumulative = '';
    let crumbs = `<button data-act="browse-dir" data-source="${esc(b.source_id)}" data-path="" type="button">${esc(source ? source.display_name : 'root')}</button>`;
    for (const part of parts) {
      cumulative = cumulative ? `${cumulative}/${part}` : part;
      crumbs += `<span>›</span><button data-act="browse-dir" data-source="${esc(b.source_id)}" data-path="${esc(cumulative)}" type="button">${esc(part)}</button>`;
    }
    const rows = (b.entries || []).map((entry) => {
      const name = esc(entry.name);
      const action = entry.kind === 'directory'
        ? `<button data-act="browse-dir" data-source="${esc(b.source_id)}" data-path="${esc(entry.relative_path)}" type="button">Open</button>`
        : `<button class="primary" data-act="add-file" data-source="${esc(b.source_id)}" data-relative="${esc(entry.relative_path)}" data-name="${esc(entry.name)}" data-size="${entry.size || 0}" type="button">Add layer</button>`;
      const size = entry.kind === 'directory' ? '—' : `${Number(entry.size || 0).toLocaleString()} B`;
      return `<tr><td class="name">${entry.kind === 'directory' ? '📁 ' : ''}${name}<div class="meta">${esc(entry.format || entry.kind)}</div></td><td>${size}</td><td>${action}</td></tr>`;
    }).join('');
    const totalPages = Math.max(1, Math.ceil((b.total || 0) / (b.page_size || 50)));
    panel.innerHTML = `<div class="card"><div class="breadcrumb">${crumbs}</div>
      <table class="table"><thead><tr><th>Name</th><th>Size</th><th></th></tr></thead><tbody>${rows || '<tr><td colspan="3" class="meta">This directory has no supported data files.</td></tr>'}</tbody></table>
      <div class="row end"><span class="meta">${b.total || 0} entries</span><button data-act="browse-page" data-source="${esc(b.source_id)}" data-path="${esc(b.path)}" data-page="${Math.max(0, (b.page || 0) - 1)}" ${b.page ? '' : 'disabled'} type="button">Previous</button>
      <span class="meta">page ${(b.page || 0) + 1} / ${totalPages}</span><button data-act="browse-page" data-source="${esc(b.source_id)}" data-path="${esc(b.path)}" data-page="${(b.page || 0) + 1}" ${b.page + 1 < totalPages ? '' : 'disabled'} type="button">Next</button></div></div>`;
  }
  function requireOpenProject() {
    if (!state.current) {
      toast('Open a project or working copy before adding a layer', true);
      return false;
    }
    return true;
  }
  function requireEditableProject() {
    if (!requireOpenProject()) return false;
    if (!state.current.workingId) {
      toast('Create a working copy before adding a layer', true);
      return false;
    }
    return true;
  }
  async function addFileLayer(sourceId, relative, name, size) {
    if (!requireEditableProject()) return;
    if (!state.webReady) { toast('The map build is missing; cannot add a layer', true); return; }
    if (isTiffName(name) && Number(size || 0) > MAX_AUTO_TIFF_BYTES) {
      toast(`${name} is ${formatBytes(size)}. The source browser refuses TIFFs over 64 MiB because a non-COG TIFF is downloaded whole; add a known COG from the map's Add Data From URL instead, or convert this file to a COG.`, true);
      return;
    }
    const rel = relative.split('/').map(enc).join('/');
    const url = `${location.origin}/api/files/${enc(sourceId)}/${rel}`;
    try {
      await runCommand('ui.addUrl', { url, name, zoom: true });
      toast(`Requested ${name}. Large files are read with HTTP Range when the provider supports it.`);
    } catch (e) { toast(`Could not add layer: ${errorText(e)}`, true); }
  }

  // ---- databases ----------------------------------------------------------
  async function refreshDatabases() {
    const data = await api('/api/sources/databases');
    state.databaseViews = data.connections || [];
    renderDatabases();
  }
  function renderDatabases() {
    const el = $('database-list');
    const list = state.databaseViews || [];
    if (!list.length) { el.innerHTML = '<p class="meta">No database connections yet.</p>'; return; }
    el.innerHTML = list.map((d) => `<div class="card"><h3>${esc(d.display_name)}</h3><div class="meta mono">${esc(d.username)}@${esc(d.host)}:${esc(d.port)}${d.database ? `/${esc(d.database)}` : ''} · ssl ${esc(d.ssl_mode)}</div>
      <div class="row" style="margin-top:5px"><span class="badge ${d.password_required ? 'warn' : 'ok'}">${d.password_required ? 'password required' : 'password in memory'}</span>
      <button data-act="db-names" data-id="${esc(d.id)}" type="button">Browse databases</button>
      <button class="danger" data-act="db-remove" data-id="${esc(d.id)}" type="button">Remove</button></div>
      ${d.password_required ? `<div class="row" style="margin-top:5px"><input type="password" autocomplete="off" data-password="${esc(d.id)}" placeholder="Password (kept in server memory only)"><button data-act="db-password" data-id="${esc(d.id)}" type="button">Use</button></div>` : ''}</div>`).join('');
  }
  async function addDatabase() {
    const host = $('db-host').value.trim();
    const port = Number($('db-port').value || 5432);
    const username = $('db-user').value.trim();
    const password = $('db-password').value;
    const database = $('db-name').value.trim();
    const sslMode = $('db-ssl').value;
    const displayName = $('db-label').value.trim();
    if (!host || !username) { toast('Host and username are required', true); return; }
    try {
      await api('/api/sources/databases', { method: 'POST', body: {
        host, port, username, sslMode, displayName: displayName || null, database: database || null, password: password || null,
      } });
      $('db-password').value = ''; $('db-label').value = '';
      await refreshDatabases();
      toast('Connection saved. The password is held in server memory only.');
    } catch (e) { toast(`Could not add connection: ${errorText(e)}`, true); }
  }
  async function databaseAction(button) {
    const act = button.dataset.act;
    const id = button.dataset.id;
    try {
      if (act === 'db-password') {
        const input = document.querySelector(`input[data-password="${CSS.escape(id)}"]`);
        const password = input ? input.value : '';
        if (!password) { toast('Enter the password first', true); return; }
        await api(`/api/sources/databases/${enc(id)}/password`, { method: 'POST', body: { password } });
        if (input) input.value = '';
        await refreshDatabases();
        toast('Password accepted for this process');
      } else if (act === 'db-remove') {
        if (!window.confirm('Remove this database connection? No data is deleted.')) return;
        await api(`/api/sources/databases/${enc(id)}`, { method: 'DELETE' });
        state.catalog = null; state.dbNames = null; $('database-catalog').style.display = 'none';
        await refreshDatabases();
      } else if (act === 'db-names') {
        const data = await api(`/api/sources/databases/${enc(id)}/databases`);
        state.dbNames = { id, names: data.databases || [] };
        state.catalog = null;
        renderDatabaseNames();
      } else if (act === 'db-catalog') {
        const db = button.dataset.database;
        state.selectedDb = db;
        const data = await api(`/api/sources/databases/${enc(id)}/catalog?database=${enc(db)}`);
        state.catalog = { id, data };
        renderCatalog();
      } else if (act === 'db-snapshot') {
        const db = button.dataset.database;
        const table = button.dataset.table;
        const name = button.dataset.name;
        if (!requireEditableProject()) return;
        if (!state.webReady) { toast('The map build is missing; cannot add a layer', true); return; }
        const url = `${location.origin}/api/sources/databases/${enc(id)}/geojson?database=${enc(db)}&table=${enc(table)}&limit=5000`;
        await runCommand('ui.addUrl', { url, name: `${name} (snapshot)`, zoom: true });
        toast('Requested a bounded PostGIS snapshot. Re-add the table to refresh it.');
      }
    } catch (e) { toast(`Database operation failed: ${errorText(e)}`, true); }
  }
  function renderDatabaseNames() {
    const panel = $('database-catalog');
    if (!state.dbNames) { panel.style.display = 'none'; return; }
    panel.style.display = 'block';
    const names = state.dbNames.names || [];
    panel.innerHTML = `<div class="card"><h3>Databases</h3><div class="meta">Each database is opened separately; the server does not assume one connection can see all databases.</div>
      <div class="stack" style="margin-top:6px">${names.length ? names.map((n) => `<div class="row"><span class="mono grow">${esc(n)}</span><button data-act="db-catalog" data-id="${esc(state.dbNames.id)}" data-database="${esc(n)}" type="button">Catalog</button></div>`).join('') : '<span class="meta">No accessible databases.</span>'}</div></div>`;
  }
  function renderCatalog() {
    const panel = $('database-catalog');
    if (!state.catalog) { panel.style.display = 'none'; return; }
    panel.style.display = 'block';
    const { id, data } = state.catalog;
    const schemas = data.schemas || [];
    const body = schemas.map((schema) => {
      const rels = (schema.relations || []).map((rel) => {
        const cols = (rel.geometry_columns || []).map((col) => {
          const base = rel.schema === 'public' ? rel.name : `${rel.schema}.${rel.name}`;
          const table = col.several ? `${base}:${col.name}` : base;
          return `<button data-act="db-snapshot" data-id="${esc(id)}" data-database="${esc(data.database)}" data-table="${esc(table)}" data-name="${esc(base)}" type="button">+ ${esc(col.name)} · ${esc(col.kind)}${col.srid ? ` · EPSG:${esc(col.srid)}` : ''}</button>`;
        }).join('');
        return `<div class="rel"><div class="name">${esc(rel.name)} <span class="badge">${esc(rel.kind)}</span></div><div class="cols">${cols || '<span class="meta">no geometry column</span>'}</div></div>`;
      }).join('');
      return `<div class="schema"><h4>${esc(schema.name)}</h4>${rels || '<div class="meta">no relations</div>'}</div>`;
    }).join('');
    panel.innerHTML = `<div class="card"><div class="row"><h3 class="grow">${esc(data.database)}</h3><button data-act="db-names" data-id="${esc(id)}" type="button">Databases</button></div>
      <div class="meta">${data.postgis_available ? 'PostGIS detected.' : 'PostGIS extension not detected.'} Geometry layers are added as bounded GeoJSON snapshots because a browser cannot connect to PostgreSQL directly.</div>
      <div class="tree">${body || '<p class="meta">No schemas or relations.</p>'}</div></div>`;
  }

  // ---- event wiring -------------------------------------------------------
  for (const tab of document.querySelectorAll('.tabs button')) {
    tab.addEventListener('click', () => {
      document.querySelectorAll('.tabs button').forEach((b) => b.classList.toggle('active', b === tab));
      document.querySelectorAll('.panel').forEach((p) => p.classList.toggle('active', p.id === `panel-${tab.dataset.tab}`));
    });
  }
  $('refresh-all').addEventListener('click', () => refreshAll());
  $('create-project').addEventListener('click', createProject);
  $('new-project-name').addEventListener('keydown', (e) => { if (e.key === 'Enter') createProject(); });
  $('copy-version').addEventListener('click', copyCurrentVersion);
  $('save-version').addEventListener('click', saveWorkingCopy);
  $('recheck').addEventListener('click', recheckAvailability);
  $('add-source').addEventListener('click', addSource);
  $('add-database').addEventListener('click', addDatabase);

  $('project-list').addEventListener('click', (e) => {
    const b = e.target.closest('button[data-act]');
    if (!b) return;
    if (b.dataset.act === 'open-version') openVersion(b.dataset.project, b.dataset.version);
    else if (b.dataset.act === 'copy-version') {
      if (!confirmDiscard()) return;
      api(`/api/projects/${enc(b.dataset.project)}/copies`, { method: 'POST', body: { versionId: b.dataset.version } })
        .then((w) => { state.dirty = false; return refreshProjects().then(() => openWorking(b.dataset.project, w.id)); })
        .catch((err) => toast(`Could not copy version: ${errorText(err)}`, true));
    } else if (b.dataset.act === 'open-working') openWorking(b.dataset.project, b.dataset.working);
  });
  $('project-detail').addEventListener('click', (e) => {
    const b = e.target.closest('button[data-act]');
    if (b && b.dataset.act === 'open-working') openWorking(b.dataset.project, b.dataset.working);
  });
  $('source-list').addEventListener('click', (e) => {
    const b = e.target.closest('button[data-act]');
    if (b) sourceAction(b);
  });
  $('source-browser').addEventListener('click', (e) => {
    const b = e.target.closest('button[data-act]');
    if (!b) return;
    if (b.dataset.act === 'browse-dir') loadBrowse(b.dataset.source, b.dataset.path || '', 0);
    else if (b.dataset.act === 'browse-page') loadBrowse(b.dataset.source, b.dataset.path || '', Number(b.dataset.page || 0));
    else if (b.dataset.act === 'add-file') addFileLayer(b.dataset.source, b.dataset.relative, b.dataset.name, b.dataset.size);
  });
  $('database-list').addEventListener('click', (e) => {
    const b = e.target.closest('button[data-act]');
    if (b) databaseAction(b);
  });
  $('database-catalog').addEventListener('click', (e) => {
    const b = e.target.closest('button[data-act]');
    if (b) databaseAction(b);
  });

  async function refreshAll() {
    await loadHealth();
    const tasks = [refreshProjects(), refreshSources(), refreshDatabases()];
    const results = await Promise.allSettled(tasks);
    for (const result of results) if (result.status === 'rejected') toast(`Refresh failed: ${errorText(result.reason)}`, true);
  }

  renderMapBar();
  refreshAll();
})();
