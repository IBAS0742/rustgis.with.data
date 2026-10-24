/* RustGIS Preview console. Plain JavaScript on purpose: the map stays in the RustGIS WASM app,
   while this page owns server-specific state (immutable versions, source roots, DB passwords).

   Interaction model:
   - The page opens with a blank scratch map and no project. Layers can be added freely; nothing
     is persisted until the user saves. Closing the page with unsaved work warns first.
   - Saving a scratch asks for project info (name/description) and creates V1; editing then
     continues in a working copy whose saves publish new immutable versions. */
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
    scratchContent: false,
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
  const STATUS_LABELS = {
    available: '可用', unavailable: '不可用', unknown: '未知',
    ready: '已就绪', error: '错误', disabled: '已停用', enabled: '已启用',
  };
  function statusLabel(status) {
    return STATUS_LABELS[status] || status;
  }
  function badge(status) {
    const cls = status === 'available' || status === 'ready' ? 'ok' : status === 'unavailable' || status === 'error' ? 'bad' : 'warn';
    return `<span class="badge ${cls}">${esc(statusLabel(status))}</span>`;
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
  function fmtTime(ms) {
    if (!ms) return '—';
    return new Date(ms).toLocaleString('zh-CN', { hour12: false });
  }
  function projectHasContent(text) {
    try {
      const value = JSON.parse(text);
      return (value.maps || []).some((m) => ((m.layers || []).length + (m.tiles || []).length) > 0);
    } catch {
      return false;
    }
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

  // ---- modal dialog -------------------------------------------------------
  function closeModal() {
    $('modal').style.display = 'none';
    $('modal-fields').innerHTML = '';
  }
  // fields: [{ id, label, placeholder, value, required, multiline }]
  function openModal({ title, fields, okText, onOk }) {
    $('modal-heading').textContent = title;
    $('modal-fields').innerHTML = fields.map((f) => `
      <div class="field">
        <label for="modal-${f.id}">${esc(f.label)}</label>
        ${f.multiline
          ? `<textarea id="modal-${f.id}" placeholder="${esc(f.placeholder || '')}">${esc(f.value || '')}</textarea>`
          : `<input id="modal-${f.id}" placeholder="${esc(f.placeholder || '')}" value="${esc(f.value || '')}">`}
      </div>`).join('');
    const ok = $('modal-ok');
    ok.textContent = okText || '确定';
    const run = async () => {
      const values = {};
      for (const f of fields) {
        const el = $(`modal-${f.id}`);
        values[f.id] = el ? el.value.trim() : '';
        if (f.required && !values[f.id]) {
          toast(`请填写「${f.label.replace(/(（.*）|\(.*\))/, '')}」`, true);
          el.focus();
          return;
        }
      }
      closeModal();
      try {
        await onOk(values);
      } catch (e) {
        toast(`操作失败:${errorText(e)}`, true);
      }
    };
    ok.onclick = run;
    $('modal-cancel').onclick = closeModal;
    $('modal').style.display = 'flex';
    const first = $('modal-fields').querySelector('input,textarea');
    if (first) first.focus();
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
      if (state.current) {
        state.dirty = state.loadedText !== null && m.text !== state.loadedText;
      } else {
        // Scratch: any layer or tile means there is something unsaved to warn about.
        state.scratchContent = projectHasContent(m.text);
      }
      renderMapBar();
    } else if (m.rustgis === 'result') {
      const waiter = state.waiters.get(m.id);
      if (!waiter) return;
      state.waiters.delete(m.id);
      if (m.ok) waiter.resolve(m.value);
      else waiter.reject(new Error(m.error || 'map command failed'));
    }
  });

  // Requirement: closing the page with unsaved work (scratch content or dirty working copy)
  // must warn. Scratch work is not recoverable after closing.
  window.addEventListener('beforeunload', (event) => {
    const unsaved = state.current ? state.dirty : state.scratchContent;
    if (unsaved) {
      event.preventDefault();
      event.returnValue = '';
    }
  });

  // ---- health and project state ------------------------------------------
  async function loadHealth() {
    try {
      state.health = await api('/api/health');
      state.webReady = !!(state.health && state.health.web_ready);
      $('health-dot').className = 'dot ok';
      $('health-text').textContent = state.webReady ? '服务在线 · 地图可用' : '服务在线 · 缺少地图构建';
      $('map-overlay').style.display = state.webReady ? 'none' : 'flex';
    } catch (e) {
      $('health-dot').className = 'dot bad';
      $('health-text').textContent = '服务不可用';
      toast(`无法连接服务:${errorText(e)}`, true);
    }
  }

  function renderMapBar() {
    const c = state.current;
    const scratch = !c;
    $('map-project').textContent = scratch ? '未保存的草稿' : c.project.name;
    $('map-mode').textContent = scratch ? '草稿 · 关闭前请保存' : c.workingId ? '工作副本 · 可编辑' : '只读版本';
    $('map-mode').className = `badge ${scratch ? '' : c.workingId ? 'ok' : 'warn'}`;
    $('map-dirty').textContent = scratch
      ? (state.scratchContent ? '有未保存的内容' : '')
      : state.dirty ? '有未保存的修改' : '已保存';
    $('copy-version').style.display = scratch ? 'none' : '';
    $('save-version').style.display = scratch ? 'none' : '';
    $('save-scratch').style.display = scratch ? '' : 'none';
    $('copy-version').disabled = !c || !!c.workingId;
    $('save-version').disabled = !c || !c.workingId || !state.webReady;
    $('save-scratch').disabled = !state.scratchContent || !state.webReady;
  }

  function renderProjectDetail() {
    const c = state.current;
    const el = $('project-detail');
    if (!c) { el.style.display = 'none'; el.innerHTML = ''; return; }
    el.style.display = 'block';
    const base = c.workingId
      ? `<div class="meta">工作副本 <span class="mono">${esc(c.workingId)}</span>${c.baseVersion ? ` · 基于版本 ${esc(c.baseVersion)}` : ''}</div>`
      : `<div class="meta">版本 <span class="mono">${esc(c.versionId || '')}</span> · 不可变(只读)</div>`;
    el.innerHTML = `<div class="card"><h3>${esc(c.project.name)}</h3>
      ${c.project.description ? `<div class="proj-desc">${esc(c.project.description)}</div>` : ''}
      <div class="meta">工程 <span class="mono">${esc(c.project.id)}</span></div>${base}
      <div class="row" style="margin-top:7px">${c.workingId ? `<button data-act="open-working" data-project="${esc(c.project.id)}" data-working="${esc(c.workingId)}" type="button">重新加载工作副本</button>` : ''}</div></div>`;
  }

  function availabilityHtml(report) {
    if (!report) return '<p class="meta">暂无可用性报告。</p>';
    const layers = report.layers || [];
    if (!layers.length) return '<p class="meta">该工程没有图层。</p>';
    return layers.map((l) => {
      const name = l.name || `图层 ${l.layer_id}`;
      return `<div class="layerstatus"><span><strong>${esc(name)}</strong> <span class="muted">· ${esc(l.map_name)}</span></span>${badge(l.status)}<span class="reason">${esc(l.reason || '')}${l.source_id ? ` · 数据源 ${esc(l.source_id)}` : ''}</span></div>`;
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
      el.innerHTML = '<p class="meta">还没有工程。直接在地图上操作后点「保存为工程」,或在上方输入名称新建。</p>';
      return;
    }
    // The server lists projects by creation time (newest first); cards keep that order.
    el.innerHTML = state.projects.map((p) => {
      const versionCount = (p.versions || []).length;
      const versions = (p.versions || []).map((v, i) => ({ ...v, label: `V${i + 1}` }));
      const idToLabel = new Map(versions.map((v) => [v.id, v.label]));
      const versionRows = versions.slice().reverse().map((v) =>
        `<div class="version"><div class="row"><strong class="vtitle">${esc(v.title || v.label)}</strong><span class="badge">${esc(v.label)}</span>${state.current && state.current.versionId === v.id ? '<span class="badge ok">打开中</span>' : ''}</div>
         <div class="meta"><span class="time">${fmtTime(v.created_at_ms)}</span> · ${v.size || 0} 字节${v.parent ? ` · 基于 ${esc(idToLabel.get(v.parent) || v.parent)}` : ''} · <span class="mono">${esc(v.id)}</span></div>
         <div class="row"><button data-act="open-version" data-project="${esc(p.id)}" data-version="${esc(v.id)}" type="button">只读打开</button>
         <button data-act="copy-version" data-project="${esc(p.id)}" data-version="${esc(v.id)}" type="button">复制为工作副本</button></div></div>`).join('');
      const workingRows = (p.working_copies || []).map((w) =>
        `<div class="version"><div class="row"><strong class="vtitle">${esc(w.title || '工作副本')}</strong><span class="badge">工作副本</span>${state.current && state.current.workingId === w.id ? '<span class="badge ok">打开中</span>' : ''}</div>
         <div class="meta">基于 ${esc(idToLabel.get(w.base_version) || w.base_version || '—')} · <span class="time">${fmtTime(w.updated_at_ms)}</span> · <span class="mono">${esc(w.id)}</span></div>
         <div class="row"><button data-act="open-working" data-project="${esc(p.id)}" data-working="${esc(w.id)}" type="button">打开工作副本</button></div></div>`).join('');
      return `<div class="card"><h3>${esc(p.name)}</h3>
        ${p.description ? `<div class="proj-desc">${esc(p.description)}</div>` : ''}
        <div class="meta"><span class="time">创建于 ${fmtTime(p.created_at_ms)}</span> · 更新于 ${fmtTime(p.updated_at_ms)}</div>
        <div class="meta">工程 <span class="mono">${esc(p.id)}</span> · ${versionCount} 个版本 · ${(p.working_copies || []).length} 个工作副本</div>
        <div class="row" style="margin:6px 0 4px"><button data-act="edit-project" data-project="${esc(p.id)}" type="button">编辑信息</button></div>
        ${versionRows}${workingRows}</div>`;
    }).join('');
  }

  function confirmDiscard() {
    return !state.dirty || window.confirm('当前地图有未保存的修改,放弃并继续?');
  }

  async function openVersion(projectId, versionId) {
    if (!confirmDiscard()) return;
    try {
      const data = await api(`/api/projects/${enc(projectId)}/versions/${enc(versionId)}/prepared`);
      state.current = { project: data.project, versionId, workingId: null, readOnly: true };
      state.availability = data.availability;
      state.loadedText = data.content;
      state.dirty = false;
      state.scratchContent = false;
      sendProject(data.content);
      renderProjectDetail(); renderAvailability(); renderMapBar(); renderProjects();
      toast('已只读打开版本;要修改请点击「复制为工作副本」');
    } catch (e) { toast(`打开版本失败:${errorText(e)}`, true); }
  }
  async function openWorking(projectId, workingId) {
    if (!confirmDiscard()) return;
    try {
      const data = await api(`/api/projects/${enc(projectId)}/working/${enc(workingId)}/prepared`);
      state.current = { project: data.project, versionId: null, workingId, baseVersion: data.base_version || null, readOnly: false };
      state.availability = data.availability;
      state.loadedText = data.content;
      state.dirty = false;
      state.scratchContent = false;
      sendProject(data.content);
      renderProjectDetail(); renderAvailability(); renderMapBar(); renderProjects();
      toast('已打开工作副本;「保存新版本」会生成新的不可变版本');
    } catch (e) { toast(`打开工作副本失败:${errorText(e)}`, true); }
  }

  async function createProject() {
    const name = $('new-project-name').value.trim();
    if (!name) { toast('请先输入工程名称', true); return; }
    try {
      // The server persists an empty immutable V1 as the baseline, then we drop the user
      // straight into an editable working copy of it.
      const created = await api('/api/projects', { method: 'POST', body: { name, content: null } });
      $('new-project-name').value = '';
      const w = await api(`/api/projects/${enc(created.project.id)}/copies`, { method: 'POST', body: { versionId: created.version.id } });
      await refreshProjects();
      await openWorking(created.project.id, w.id);
      toast(`已创建工程「${name}」并进入工作副本`);
    } catch (e) { toast(`创建工程失败:${errorText(e)}`, true); }
  }

  // Requirement: saving a scratch asks for project info instead of using a generated ID.
  async function saveScratchAsProject() {
    if (!state.scratchContent || !state.projectText) { toast('草稿还没有任何内容,先添加一个图层吧', true); return; }
    openModal({
      title: '保存为工程',
      fields: [
        { id: 'name', label: '工程名称(必填)', placeholder: '例如:2026 年测绘底图', required: true },
        { id: 'description', label: '备注 / 描述(可选)', placeholder: '用途、数据来源、范围等', multiline: true },
      ],
      okText: '保存',
      onOk: async (values) => {
        const created = await api('/api/projects', {
          method: 'POST',
          body: { name: values.name, description: values.description || null, content: state.projectText },
        });
        const w = await api(`/api/projects/${enc(created.project.id)}/copies`, { method: 'POST', body: { versionId: created.version.id } });
        await refreshProjects();
        await openWorking(created.project.id, w.id);
        toast(`已保存为工程「${values.name}」,可继续编辑`);
      },
    });
  }

  // Requirement: copies ask for a user-facing title instead of showing a raw ID.
  function copyVersionDialog(projectId, versionId) {
    if (!confirmDiscard()) return;
    openModal({
      title: '复制为工作副本',
      fields: [{ id: 'title', label: '副本标题(可选)', placeholder: '例如:野外调绘副本', value: '' }],
      okText: '复制',
      onOk: async (values) => {
        const w = await api(`/api/projects/${enc(projectId)}/copies`, { method: 'POST', body: { versionId, title: values.title || null } });
        state.dirty = false;
        await refreshProjects();
        await openWorking(projectId, w.id);
        toast(values.title ? `已创建工作副本「${values.title}」` : '已创建工作副本,保存时会发布新版本');
      },
    });
  }
  function copyCurrentVersion() {
    const c = state.current;
    if (!c || !c.versionId) { toast('请先只读打开一个版本', true); return; }
    copyVersionDialog(c.project.id, c.versionId);
  }

  async function saveWorkingCopy() {
    const c = state.current;
    if (!c || !c.workingId) { toast('请先打开工作副本再保存', true); return; }
    if (!state.projectText) { toast('地图尚未返回工程内容', true); return; }
    openModal({
      title: '保存为新版本',
      fields: [{ id: 'title', label: '版本标题(可选)', placeholder: '例如:调绘成果版', value: '' }],
      okText: '保存',
      onOk: async (values) => {
        // The map is showing the derived, availability-prepared copy. Store the original
        // visibility for layers the check hid, so the saved version can recover when the data
        // returns; opening it will still hide them while they are unavailable.
        const content = restoreUnavailableVisibility(state.projectText, state.availability);
        const version = await api(`/api/projects/${enc(c.project.id)}/working/${enc(c.workingId)}/versions`, {
          method: 'POST', body: { content, title: values.title || null },
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
        toast(`已发布不可变版本 ${values.title ? `「${values.title}」` : ''}(${version.id})`);
      },
    });
  }

  // Requirement: project info (title, remark/description) is editable; times are not.
  function editProjectInfo(projectId) {
    const p = state.projects.find((x) => x.id === projectId);
    if (!p) return;
    openModal({
      title: '编辑工程信息',
      fields: [
        { id: 'name', label: '工程名称(必填)', value: p.name, required: true },
        { id: 'description', label: '备注 / 描述', value: p.description || '', multiline: true },
      ],
      okText: '保存',
      onOk: async (values) => {
        await api(`/api/projects/${enc(projectId)}`, { method: 'PATCH', body: { name: values.name, description: values.description } });
        if (state.current && state.current.project.id === projectId) {
          try { state.current.project = await api(`/api/projects/${enc(projectId)}`); } catch { /* best effort */ }
          renderProjectDetail();
          renderMapBar();
        }
        await refreshProjects();
        toast('工程信息已更新(创建时间不可修改)');
      },
    });
  }

  async function recheckAvailability() {
    const c = state.current;
    if (!c || !state.projectText) { toast('请先打开工程', true); return; }
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
      toast(data.availability.unavailable ? `${data.availability.unavailable} 个图层不可用,已隐藏` : '所有引用均可访问');
    } catch (e) { toast(`重新检查失败:${errorText(e)}`, true); }
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
      el.innerHTML = '<p class="meta">还没有配置数据源目录。请在上方添加服务器上的目录。</p>';
      return;
    }
    el.innerHTML = state.sources.map((s) => {
      const status = s.scan_status === 'ready' ? 'ok' : s.scan_status === 'unavailable' || s.scan_status === 'error' ? 'bad' : 'warn';
      return `<div class="card"><h3>${esc(s.display_name)}</h3><div class="mono">${esc(s.root_path)}</div>
        <div class="row" style="margin-top:5px"><span class="badge ${status}">${esc(statusLabel(s.scan_status))}</span><span class="badge">${s.enabled ? '已启用' : '已停用'}</span>
        <button data-act="browse-source" data-id="${esc(s.id)}" type="button">浏览</button>
        <button data-act="scan-source" data-id="${esc(s.id)}" type="button">重新扫描</button>
        <button data-act="toggle-source" data-id="${esc(s.id)}" data-enabled="${s.enabled ? '1' : '0'}" type="button">${s.enabled ? '停用' : '启用'}</button>
        <button class="danger" data-act="remove-source" data-id="${esc(s.id)}" type="button">移除</button></div>
        ${s.last_error ? `<div class="meta" style="color:var(--danger)">${esc(s.last_error)}</div>` : ''}</div>`;
    }).join('');
  }

  async function addSource() {
    const path = $('source-path').value.trim();
    const name = $('source-name').value.trim();
    if (!path) { toast('请输入服务器上已存在的目录', true); return; }
    try {
      await api('/api/sources/folders', { method: 'POST', body: { path, displayName: name || null } });
      $('source-path').value = ''; $('source-name').value = '';
      await refreshSources();
      toast('已添加数据源目录。新发现的文件可浏览,不会自动加入任何工程。');
    } catch (e) { toast(`添加目录失败:${errorText(e)}`, true); }
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
        toast(summary.error ? `重新扫描报告:${summary.error}` : `重新扫描:${summary.files} 个文件,${summary.directories} 个目录`, !!summary.error);
      } else if (act === 'toggle-source') {
        await api(`/api/sources/folders/${enc(id)}`, { method: 'PATCH', body: { enabled: button.dataset.enabled !== '1' } });
        await refreshSources();
      } else if (act === 'remove-source') {
        if (!window.confirm('移除该数据源目录?磁盘上的文件不会被删除。')) return;
        await api(`/api/sources/folders/${enc(id)}`, { method: 'DELETE' });
        state.browse = null;
        $('source-browser').style.display = 'none';
        await refreshSources();
        toast('已移除数据源');
      }
    } catch (e) { toast(`数据源操作失败:${errorText(e)}`, true); }
  }

  async function loadBrowse(sourceId, path, page) {
    try {
      const data = await api(`/api/sources/folders/${enc(sourceId)}/items?path=${enc(path)}&page=${page}&page_size=50`);
      state.browse = data;
      renderBrowse();
    } catch (e) { toast(`浏览失败:${errorText(e)}`, true); }
  }
  function renderBrowse() {
    const b = state.browse;
    const panel = $('source-browser');
    if (!b) { panel.style.display = 'none'; return; }
    panel.style.display = 'block';
    const source = state.sources.find((s) => s.id === b.source_id);
    const parts = b.path ? b.path.split('/').filter(Boolean) : [];
    let cumulative = '';
    let crumbs = `<button data-act="browse-dir" data-source="${esc(b.source_id)}" data-path="" type="button">${esc(source ? source.display_name : '根目录')}</button>`;
    for (const part of parts) {
      cumulative = cumulative ? `${cumulative}/${part}` : part;
      crumbs += `<span>›</span><button data-act="browse-dir" data-source="${esc(b.source_id)}" data-path="${esc(cumulative)}" type="button">${esc(part)}</button>`;
    }
    const rows = (b.entries || []).map((entry) => {
      const name = esc(entry.name);
      const action = entry.kind === 'directory'
        ? `<button data-act="browse-dir" data-source="${esc(b.source_id)}" data-path="${esc(entry.relative_path)}" type="button">打开</button>`
        : `<button class="primary" data-act="add-file" data-source="${esc(b.source_id)}" data-relative="${esc(entry.relative_path)}" data-name="${esc(entry.name)}" data-size="${entry.size || 0}" type="button">添加图层</button>`;
      const size = entry.kind === 'directory' ? '—' : `${Number(entry.size || 0).toLocaleString()} B`;
      return `<tr><td class="name">${entry.kind === 'directory' ? '📁 ' : ''}${name}<div class="meta">${esc(entry.format || entry.kind)}</div></td><td>${size}</td><td>${action}</td></tr>`;
    }).join('');
    const totalPages = Math.max(1, Math.ceil((b.total || 0) / (b.page_size || 50)));
    panel.innerHTML = `<div class="card"><div class="breadcrumb">${crumbs}</div>
      <table class="table"><thead><tr><th>名称</th><th>大小</th><th></th></tr></thead><tbody>${rows || '<tr><td colspan="3" class="meta">该目录没有受支持的数据文件。</td></tr>'}</tbody></table>
      <div class="row end"><span class="meta">共 ${b.total || 0} 项</span><button data-act="browse-page" data-source="${esc(b.source_id)}" data-path="${esc(b.path)}" data-page="${Math.max(0, (b.page || 0) - 1)}" ${b.page ? '' : 'disabled'} type="button">上一页</button>
      <span class="meta">第 ${(b.page || 0) + 1} / ${totalPages} 页</span><button data-act="browse-page" data-source="${esc(b.source_id)}" data-path="${esc(b.path)}" data-page="${(b.page || 0) + 1}" ${b.page + 1 < totalPages ? '' : 'disabled'} type="button">下一页</button></div></div>`;
  }
  // Requirement: layers can be added to the scratch map without any project; nothing is
  // persisted until the user saves (保存为工程 / 保存新版本).
  async function addFileLayer(sourceId, relative, name, size) {
    if (!state.webReady) { toast('缺少地图构建,无法添加图层', true); return; }
    if (isTiffName(name) && Number(size || 0) > MAX_AUTO_TIFF_BYTES) {
      toast(`${name} 大小为 ${formatBytes(size)}。超过 64 MiB 的非 COG TIFF 会被完整下载,已阻止;请先转换为 COG,或在地图中通过「Add Data From URL」添加已知 COG。`, true);
      return;
    }
    const rel = relative.split('/').map(enc).join('/');
    const url = `${location.origin}/api/files/${enc(sourceId)}/${rel}`;
    try {
      await runCommand('ui.addUrl', { url, name, zoom: true });
      toast(state.current ? `已请求加载 ${name}。` : `已把 ${name} 加入草稿;点「保存为工程」持久化。`);
    } catch (e) { toast(`添加图层失败:${errorText(e)}`, true); }
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
    if (!list.length) { el.innerHTML = '<p class="meta">还没有数据库连接。</p>'; return; }
    el.innerHTML = list.map((d) => `<div class="card"><h3>${esc(d.display_name)}</h3><div class="meta mono">${esc(d.username)}@${esc(d.host)}:${esc(d.port)}${d.database ? `/${esc(d.database)}` : ''} · ssl ${esc(d.ssl_mode)}</div>
      <div class="row" style="margin-top:5px"><span class="badge ${d.password_required ? 'warn' : 'ok'}">${d.password_required ? '需要密码' : '密码已在内存'}</span>
      <button data-act="db-names" data-id="${esc(d.id)}" type="button">浏览数据库</button>
      <button class="danger" data-act="db-remove" data-id="${esc(d.id)}" type="button">移除</button></div>
      ${d.password_required ? `<div class="row" style="margin-top:5px"><input type="password" autocomplete="off" data-password="${esc(d.id)}" placeholder="密码(仅保存在服务进程内存中)"><button data-act="db-password" data-id="${esc(d.id)}" type="button">使用</button></div>` : ''}</div>`).join('');
  }
  async function addDatabase() {
    const host = $('db-host').value.trim();
    const port = Number($('db-port').value || 5432);
    const username = $('db-user').value.trim();
    const password = $('db-password').value;
    const database = $('db-name').value.trim();
    const sslMode = $('db-ssl').value;
    const displayName = $('db-label').value.trim();
    if (!host || !username) { toast('主机和用户名必填', true); return; }
    try {
      await api('/api/sources/databases', { method: 'POST', body: {
        host, port, username, sslMode, displayName: displayName || null, database: database || null, password: password || null,
      } });
      $('db-password').value = ''; $('db-label').value = '';
      await refreshDatabases();
      toast('连接已保存。密码仅保存在服务进程内存中。');
    } catch (e) { toast(`添加连接失败:${errorText(e)}`, true); }
  }
  async function databaseAction(button) {
    const act = button.dataset.act;
    const id = button.dataset.id;
    try {
      if (act === 'db-password') {
        const input = document.querySelector(`input[data-password="${CSS.escape(id)}"]`);
        const password = input ? input.value : '';
        if (!password) { toast('请先输入密码', true); return; }
        await api(`/api/sources/databases/${enc(id)}/password`, { method: 'POST', body: { password } });
        if (input) input.value = '';
        await refreshDatabases();
        toast('密码已接受,仅本次进程有效');
      } else if (act === 'db-remove') {
        if (!window.confirm('移除该数据库连接?不会删除任何数据。')) return;
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
        if (!state.webReady) { toast('缺少地图构建,无法添加图层', true); return; }
        const url = `${location.origin}/api/sources/databases/${enc(id)}/geojson?database=${enc(db)}&table=${enc(table)}&limit=5000`;
        await runCommand('ui.addUrl', { url, name: `${name}(快照)`, zoom: true });
        toast(state.current ? '已请求有界的 PostGIS 快照。' : '已把 PostGIS 快照加入草稿;点「保存为工程」持久化。');
      }
    } catch (e) { toast(`数据库操作失败:${errorText(e)}`, true); }
  }
  function renderDatabaseNames() {
    const panel = $('database-catalog');
    if (!state.dbNames) { panel.style.display = 'none'; return; }
    panel.style.display = 'block';
    const names = state.dbNames.names || [];
    panel.innerHTML = `<div class="card"><h3>数据库列表</h3><div class="meta">每个数据库单独建立连接;服务端不会假设一个连接可以访问所有数据库。</div>
      <div class="stack" style="margin-top:6px">${names.length ? names.map((n) => `<div class="row"><span class="mono grow">${esc(n)}</span><button data-act="db-catalog" data-id="${esc(state.dbNames.id)}" data-database="${esc(n)}" type="button">查看目录</button></div>`).join('') : '<span class="meta">没有可访问的数据库。</span>'}</div></div>`;
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
        return `<div class="rel"><div class="name">${esc(rel.name)} <span class="badge">${esc(rel.kind)}</span></div><div class="cols">${cols || '<span class="meta">无几何列</span>'}</div></div>`;
      }).join('');
      return `<div class="schema"><h4>${esc(schema.name)}</h4>${rels || '<div class="meta">暂无表或视图</div>'}</div>`;
    }).join('');
    panel.innerHTML = `<div class="card"><div class="row"><h3 class="grow">${esc(data.database)}</h3><button data-act="db-names" data-id="${esc(id)}" type="button">数据库列表</button></div>
      <div class="meta">${data.postgis_available ? '已检测到 PostGIS。' : '未检测到 PostGIS 扩展。'}浏览器无法直连 PostgreSQL,几何图层以有界 GeoJSON 快照方式添加。</div>
      <div class="tree">${body || '<p class="meta">暂无 schema 或表。</p>'}</div></div>`;
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
  $('save-scratch').addEventListener('click', saveScratchAsProject);
  $('recheck').addEventListener('click', recheckAvailability);
  $('add-source').addEventListener('click', addSource);
  $('add-database').addEventListener('click', addDatabase);

  $('project-list').addEventListener('click', (e) => {
    const b = e.target.closest('button[data-act]');
    if (!b) return;
    if (b.dataset.act === 'open-version') openVersion(b.dataset.project, b.dataset.version);
    else if (b.dataset.act === 'copy-version') copyVersionDialog(b.dataset.project, b.dataset.version);
    else if (b.dataset.act === 'open-working') openWorking(b.dataset.project, b.dataset.working);
    else if (b.dataset.act === 'edit-project') editProjectInfo(b.dataset.project);
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
    for (const result of results) if (result.status === 'rejected') toast(`刷新失败:${errorText(result.reason)}`, true);
  }

  renderMapBar();
  refreshAll();
})();
