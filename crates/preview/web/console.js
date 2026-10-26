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
    dbBrowse: null,
    selectedDb: null,
    pairing: null,
    importPlan: null,
    openProjects: new Set(),
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
    if (!opts.raw && opts.body && typeof opts.body !== 'string') {
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
    $('modal').classList.remove('wide');
  }
  // Escape closes the form modal first, then the floating browser window.
  document.addEventListener('keydown', (e) => {
    if (e.key !== 'Escape') return;
    if ($('modal').style.display === 'flex') closeModal();
    else if ($('float-win').style.display === 'flex') $('float-win').style.display = 'none';
  });
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
    ok.style.display = '';
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
    $('modal-cancel').textContent = '取消';
    $('modal-cancel').onclick = closeModal;
    $('modal').style.display = 'flex';
    const first = $('modal-fields').querySelector('input,textarea');
    if (first) first.focus();
  }
  // A floating, non-blocking window: one instance, reused for the last-clicked resource.
  // It can be dragged by its header and resized at the corner; nothing behind it is blocked.
  function openFloatWindow(title, html) {
    $('fw-title').textContent = title;
    $('fw-body').innerHTML = html;
    $('float-win').style.display = 'flex';
  }
  let dbPaneObserver = null;
  function closeFloatWindow() {
    if (dbPaneObserver) {
      dbPaneObserver.disconnect();
      dbPaneObserver = null;
    }
    $('float-win').style.display = 'none';
    $('fw-body').innerHTML = '';
  }
  $('fw-close').addEventListener('click', closeFloatWindow);
  // Minimize collapses the window to its title bar; the button restores it.
  $('fw-min').addEventListener('click', () => {
    const win = $('float-win');
    const min = win.classList.toggle('min');
    if (min) {
      win.dataset.prevH = win.style.height || '520px';
      win.style.height = 'auto';
      $('fw-min').textContent = '□';
      $('fw-min').title = '恢复';
    } else {
      win.style.height = win.dataset.prevH || '520px';
      $('fw-min').textContent = '─';
      $('fw-min').title = '最小化';
    }
  });
  const DB_SCHEMA_WIDTH_KEY = 'rustgis-db-schema-w';
  const DB_TABLE_WIDTH_KEY = 'rustgis-db-table-w';
  const DB_PANE_MIN = 90;
  const DB_PANE_MAX = 480;
  const DB_PANE_DEFAULT = 150;
  const DB_DATABASE_MIN = 110;
  const DB_DIVIDER_WIDTH = 7;
  const DB_DIVIDER_TOTAL = DB_DIVIDER_WIDTH * 2;

  function savedPaneWidth(key) {
    const w = Number(localStorage.getItem(key));
    return Number.isFinite(w) && w >= DB_PANE_MIN && w <= DB_PANE_MAX ? w : DB_PANE_DEFAULT;
  }
  function dbPanes(db3) {
    return [...db3.children].filter((el) => el.classList.contains('db-pane'));
  }
  // The database pane is flexible; schema and table have explicit widths. Fit both fixed panes
  // into the window while leaving the database pane its minimum, so a wide saved width can never
  // push the table pane outside the floating window.
  function fitPaneWidths(db3, wantedSchema, wantedTable) {
    const panes = dbPanes(db3);
    if (panes.length < 3) return;
    const [database, schema, table] = panes;
    const budget = db3.clientWidth - DB_DIVIDER_TOTAL;
    if (budget <= 0) return;
    const room = budget - DB_DATABASE_MIN;
    let schemaW = Math.min(DB_PANE_MAX, Math.max(DB_PANE_MIN, Math.round(wantedSchema)));
    let tableW = Math.min(DB_PANE_MAX, Math.max(DB_PANE_MIN, Math.round(wantedTable)));
    if (room < DB_PANE_MIN * 2) {
      // A very narrow window keeps the minimum widths and .db3 scrolls horizontally.
      schemaW = DB_PANE_MIN;
      tableW = DB_PANE_MIN;
    } else if (schemaW + tableW > room) {
      // Shrink by each pane's headroom above the minimum: the pane the user widened most
      // gives up the most width first.
      const excess = schemaW + tableW - room;
      const headroom = schemaW - DB_PANE_MIN + (tableW - DB_PANE_MIN);
      if (headroom > 0) {
        schemaW -= excess * ((schemaW - DB_PANE_MIN) / headroom);
        tableW -= excess * ((tableW - DB_PANE_MIN) / headroom);
      } else {
        schemaW = room / 2;
        tableW = room / 2;
      }
      schemaW = Math.max(DB_PANE_MIN, Math.round(schemaW));
      tableW = Math.max(DB_PANE_MIN, Math.round(tableW));
      const over = schemaW + tableW - room;
      if (over > 0) {
        if (schemaW >= tableW) schemaW = Math.max(DB_PANE_MIN, schemaW - over);
        else tableW = Math.max(DB_PANE_MIN, tableW - over);
      }
    }
    database.style.minWidth = `${DB_DATABASE_MIN}px`;
    schema.style.minWidth = `${DB_PANE_MIN}px`;
    table.style.minWidth = `${DB_PANE_MIN}px`;
    schema.style.width = `${schemaW}px`;
    table.style.width = `${tableW}px`;
  }
  function watchPaneWidths(db3) {
    if (dbPaneObserver) dbPaneObserver.disconnect();
    if (typeof ResizeObserver !== 'function') return;
    // The floating window's corner grip changes .db3's width without a window resize event.
    dbPaneObserver = new ResizeObserver(() => {
      fitPaneWidths(db3, savedPaneWidth(DB_SCHEMA_WIDTH_KEY), savedPaneWidth(DB_TABLE_WIDTH_KEY));
    });
    dbPaneObserver.observe(db3);
  }
  function savePaneWidth(key, width) {
    try { localStorage.setItem(key, String(width)); } catch { /* storage may be blocked */ }
  }
  function bindPaneDivider(div) {
    const db3 = div.closest('.db3');
    if (!db3) return;
    const beforeSchema = div.dataset.div === 'schema';
    const key = beforeSchema ? DB_SCHEMA_WIDTH_KEY : DB_TABLE_WIDTH_KEY;
    div.addEventListener('pointerdown', (e) => {
      if (e.button !== 0) return;
      const panes = dbPanes(db3);
      if (panes.length < 3) return;
      const [, schema, table] = panes;
      e.preventDefault();
      div.setPointerCapture(e.pointerId);
      div.classList.add('dragging');
      document.body.style.userSelect = 'none';
      const startX = e.clientX;
      const startSchema = schema.getBoundingClientRect().width;
      const startTable = table.getBoundingClientRect().width;
      const total = startSchema + startTable;
      const onMove = (ev) => {
        const dx = ev.clientX - startX;
        if (beforeSchema) {
          // Move the database/schema boundary with the pointer: the schema narrows when the
          // pointer moves right and the flexible database pane takes exactly that width.
          const room = db3.clientWidth - DB_DIVIDER_TOTAL - DB_DATABASE_MIN;
          const maxSchema = Math.max(DB_PANE_MIN, Math.min(DB_PANE_MAX, room - startTable));
          const schemaW = Math.min(maxSchema, Math.max(DB_PANE_MIN, Math.round(startSchema - dx)));
          schema.style.width = `${schemaW}px`;
          savePaneWidth(key, schemaW);
        } else {
          // Keep schema + table constant so the database pane (and the first divider) stays
          // still: moving right widens schema and narrows the table by the same amount.
          const minSchema = Math.max(DB_PANE_MIN, total - DB_PANE_MAX);
          const maxSchema = Math.min(DB_PANE_MAX, total - DB_PANE_MIN);
          const schemaW = Math.min(maxSchema, Math.max(minSchema, Math.round(startSchema + dx)));
          const tableW = Math.round(total - schemaW);
          schema.style.width = `${schemaW}px`;
          table.style.width = `${tableW}px`;
          savePaneWidth(DB_SCHEMA_WIDTH_KEY, schemaW);
          savePaneWidth(DB_TABLE_WIDTH_KEY, tableW);
        }
      };
      const finish = (ev) => {
        if (div.hasPointerCapture(ev.pointerId)) div.releasePointerCapture(ev.pointerId);
        div.classList.remove('dragging');
        document.body.style.userSelect = '';
        div.removeEventListener('pointermove', onMove);
        div.removeEventListener('pointerup', finish);
        div.removeEventListener('pointercancel', finish);
      };
      div.addEventListener('pointermove', onMove);
      div.addEventListener('pointerup', finish);
      div.addEventListener('pointercancel', finish);
    });
    // Double-clicking a divider restores that pane's default width; the other pane keeps its
    // saved width and fitPaneWidths shrinks both only if the window is too narrow.
    div.addEventListener('dblclick', () => {
      savePaneWidth(key, DB_PANE_DEFAULT);
      fitPaneWidths(db3, savedPaneWidth(DB_SCHEMA_WIDTH_KEY), savedPaneWidth(DB_TABLE_WIDTH_KEY));
    });
  }
  function dragWindow(handleEl, targetEl) {
    handleEl.addEventListener('pointerdown', (e) => {
      if (e.target.closest('button')) return; // the close button still clicks normally
      e.preventDefault();
      handleEl.setPointerCapture(e.pointerId);
      const rect = targetEl.getBoundingClientRect();
      const grabX = e.clientX - rect.left;
      const grabY = e.clientY - rect.top;
      const onMove = (ev) => {
        const x = Math.min(Math.max(0, ev.clientX - grabX), window.innerWidth - 80);
        const y = Math.min(Math.max(0, ev.clientY - grabY), window.innerHeight - 40);
        targetEl.style.left = `${x}px`;
        targetEl.style.top = `${y}px`;
      };
      const onUp = (ev) => {
        handleEl.releasePointerCapture(ev.pointerId);
        handleEl.removeEventListener('pointermove', onMove);
        handleEl.removeEventListener('pointerup', onUp);
      };
      handleEl.addEventListener('pointermove', onMove);
      handleEl.addEventListener('pointerup', onUp);
    });
  }
  dragWindow($('fw-head'), $('float-win'));
  $('fw-resize').addEventListener('pointerdown', (e) => {
    e.preventDefault();
    const corner = $('fw-resize');
    corner.setPointerCapture(e.pointerId);
    const rect = $('float-win').getBoundingClientRect();
    const onMove = (ev) => {
      const w = Math.min(Math.max(420, ev.clientX - rect.left), window.innerWidth - rect.left - 8);
      const h = Math.min(Math.max(260, ev.clientY - rect.top), window.innerHeight - rect.top - 8);
      $('float-win').style.width = `${w}px`;
      $('float-win').style.height = `${h}px`;
    };
    const onUp = (ev) => {
      corner.releasePointerCapture(ev.pointerId);
      corner.removeEventListener('pointermove', onMove);
      corner.removeEventListener('pointerup', onUp);
    };
    corner.addEventListener('pointermove', onMove);
    corner.addEventListener('pointerup', onUp);
  });

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
    const unpaired = unpairedImportLayers();
    $('pair-layers').style.display = state.current.workingId && unpaired.length ? '' : 'none';
  }

  function unpairedImportLayers() {
    const layers = (state.current && state.current.project && state.current.project.import && state.current.project.import.layers) || [];
    return layers.filter((l) => l.status === 'unpaired' || l.status === 'candidate');
  }

  // ---- layer pairing (imported projects) ----------------------------------
  function renderPairingWindow() {
    const c = state.current;
    const layers = (c && c.project && c.project.import && c.project.import.layers) || [];
    if (!c || !c.workingId || !layers.length) { toast('当前打开的工程没有可配对的导入图层', true); return; }
    const rows = layers.map((l) => {
      const status = l.status === 'paired'
        ? `<span class="badge ok">已配对</span><div class="meta">${esc(l.target || '')}</div>`
        : l.status === 'candidate'
          ? `<span class="badge warn">可配对${l.target ? `:${esc(l.target)}` : ''}</span><div class="meta">${esc(l.qgis_datasource || '')}</div>`
          : `<span class="badge bad">未配对</span><div class="meta">${esc(l.qgis_datasource || '')}</div>`;
      const actions = l.status !== 'paired'
        ? `<button class="primary" data-act="pair-file-pick" data-layer="${esc(l.name)}" type="button">配对文件…</button>
           <button data-act="pair-db-pick" data-layer="${esc(l.name)}" data-target="${esc(l.target || '')}" type="button">配对数据库表…</button>`
        : '';
      return `<tr><td class="name"><strong>${esc(l.name)}</strong><div class="meta">${esc(l.kind)} · 样式${esc(l.style_note)}</div></td>
        <td>${status}</td><td class="ti-actions">${actions}</td></tr>`;
    }).join('');
    openFloatWindow('配对导入图层', `<div class="browse-head"><div class="meta">把未配对图层连接到数据源:文件须位于已配置的数据源目录内;数据库表名和结构必须与原工程一致。配对会写入当前工作副本,保存后成为新版本。</div></div>
      <table class="table browse-table"><thead><tr><th>图层</th><th>状态 / 原数据源</th><th></th></tr></thead><tbody>${rows}</tbody></table>`);
  }

  async function openDbBrowse(connectionId) {
    const data = await api(`/api/sources/databases/${enc(connectionId)}/databases`);
    const prev = state.dbBrowse && state.dbBrowse.connId === connectionId ? state.dbBrowse : null;
    state.dbBrowse = {
      connId: connectionId,
      databases: data.databases || [],
      selectedDb: prev ? prev.selectedDb : null,
      catalog: null,
      selectedSchema: null,
    };
    renderDatabaseBrowser();
  }

  async function doPairFile(sourceId, relative) {
    const c = state.current;
    const layer = state.pairing && state.pairing.layer;
    if (!c || !c.workingId || !layer) return;
    try {
      await api(`/api/projects/${enc(c.project.id)}/working/${enc(c.workingId)}/pair`, {
        method: 'POST',
        body: { layerName: layer, file: { path: relative } },
      });
      state.pairing = null;
      closeFloatWindow();
      await openWorking(c.project.id, c.workingId, { skipConfirm: true });
      toast(`图层「${layer}」已配对文件并重新加载`);
    } catch (e) {
      toast(`配对失败:${errorText(e)}`, true);
    }
  }

  async function doPairDb(connectionId, database, tableRef) {
    const c = state.current;
    const layer = state.pairing && state.pairing.layer;
    if (!c || !c.workingId || !layer) return;
    // tableRef is `schema.table` or `schema.table:geom`; the geometry column is not needed here.
    const plain = tableRef.split(':')[0];
    const dot = plain.indexOf('.');
    const schema = dot > 0 ? plain.slice(0, dot) : '';
    const table = dot > 0 ? plain.slice(dot + 1) : plain;
    try {
      await api(`/api/projects/${enc(c.project.id)}/working/${enc(c.workingId)}/pair`, {
        method: 'POST',
        body: { layerName: layer, postgis: { connection: connectionId, database, schema: schema || null, table } },
      });
      state.pairing = null;
      closeFloatWindow();
      await openWorking(c.project.id, c.workingId, { skipConfirm: true });
      toast(`图层「${layer}」已配对数据库表并加载快照`);
    } catch (e) {
      toast(`配对失败:${errorText(e)}`, true);
    }
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
    // Two-level tree. Level 1: project group. Level 2: its versions and working copies, each
    // row showing title, time and its actions. The server lists projects by creation time.
    el.innerHTML = state.projects.map((p) => {
      const versions = (p.versions || []).map((v, i) => ({ ...v, label: `V${i + 1}` }));
      const idToLabel = new Map(versions.map((v) => [v.id, v.label]));
      const versionRows = versions.slice().reverse().map((v) => `
        <div class="tree-item">
          <span class="ti-title"><strong>${esc(v.title || v.label)}</strong><span class="badge">${esc(v.label)}</span>${state.current && state.current.versionId === v.id ? '<span class="badge ok">打开中</span>' : ''}</span>
          <span class="ti-time">${fmtTime(v.created_at_ms)}</span>
          <span class="ti-actions">
            <button data-act="open-version" data-project="${esc(p.id)}" data-version="${esc(v.id)}" type="button">只读打开</button>
            <button data-act="copy-version" data-project="${esc(p.id)}" data-version="${esc(v.id)}" type="button">以副本打开</button>
          </span>
        </div>`).join('');
      const workingRows = (p.working_copies || []).map((w) => `
        <div class="tree-item">
          <span class="ti-title"><strong>${esc(w.title || '工作副本')}</strong><span class="badge">工作副本</span>${state.current && state.current.workingId === w.id ? '<span class="badge ok">打开中</span>' : ''}</span>
          <span class="ti-time">基于 ${esc(idToLabel.get(w.base_version) || '—')} · ${fmtTime(w.updated_at_ms)}</span>
          <span class="ti-actions"><button data-act="open-working" data-project="${esc(p.id)}" data-working="${esc(w.id)}" type="button">打开工作副本</button></span>
        </div>`).join('');
      // Tree groups start collapsed; the set remembers which ones the user expanded.
      const collapsed = !state.openProjects.has(p.id);
      return `<div class="card project-card${collapsed ? ' collapsed' : ''}">
        <div class="pcard-head">
          <button class="pcard-toggle" data-act="toggle-project" data-project="${esc(p.id)}" type="button" aria-expanded="${collapsed ? 'false' : 'true'}" title="展开/收起">${collapsed ? '▶' : '▼'}</button>
          <div class="grow">
            <div class="row" style="gap:8px"><h3>${esc(p.name)}</h3><span class="pcard-count">${(p.versions || []).length} 版本 · ${(p.working_copies || []).length} 副本</span></div>
            ${p.description ? `<div class="proj-desc">${esc(p.description)}</div>` : ''}
            <div class="meta pcard-meta"><span class="time">创建于 ${fmtTime(p.created_at_ms)}</span> · 更新于 ${fmtTime(p.updated_at_ms)}</div>
          </div>
          <button data-act="edit-project" data-project="${esc(p.id)}" type="button">编辑信息</button>
        </div>
        <div class="pcard-body">${versionRows}${workingRows}</div>
      </div>`;
    }).join('');
  }

  function confirmDiscard() {
    return !state.dirty || window.confirm('当前地图有未保存的修改,放弃并继续?');
  }

  async function openVersion(projectId, versionId) {
    if (!confirmDiscard()) return;
    try {
      state.openProjects.add(projectId);
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
  // skipConfirm is used by flows that already asked (e.g. right after the copy dialog).
  async function openWorking(projectId, workingId, { skipConfirm = false } = {}) {
    if (!skipConfirm && !confirmDiscard()) return;
    try {
      state.openProjects.add(projectId);
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
        await openWorking(created.project.id, w.id, { skipConfirm: true });
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
        await openWorking(projectId, w.id, { skipConfirm: true });
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
        // Reopening a source returns to the directory the user last browsed.
        const lastPath = (state.browsePaths && state.browsePaths[id]) || '';
        await loadBrowse(id, lastPath, 0);
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
        closeFloatWindow();
        await refreshSources();
        toast('已移除数据源');
      }
    } catch (e) { toast(`数据源操作失败:${errorText(e)}`, true); }
  }

  async function loadBrowse(sourceId, path, page) {
    try {
      const data = await api(`/api/sources/folders/${enc(sourceId)}/items?path=${enc(path)}&page=${page}&page_size=50`);
      state.browse = data;
      state.browsePaths = state.browsePaths || {};
      state.browsePaths[sourceId] = path || '';
      renderBrowse();
    } catch (e) {
      toast(`浏览失败:${errorText(e)}`, true);
      closeFloatWindow();
    }
  }
  // The browser lives in the floating window so each file can show format, size, modified
  // time and its server-side relative path next to the add-layer action, without blocking
  // anything else. Clicking another source reuses the same window.
  function renderBrowse() {
    const b = state.browse;
    if (!b) { closeFloatWindow(); return; }
    const source = state.sources.find((s) => s.id === b.source_id);
    const parts = b.path ? b.path.split('/').filter(Boolean) : [];
    let cumulative = '';
    let crumbs = `<button data-act="browse-dir" data-source="${esc(b.source_id)}" data-path="" type="button">${esc(source ? source.display_name : '根目录')}</button>`;
    for (const part of parts) {
      cumulative = cumulative ? `${cumulative}/${part}` : part;
      crumbs += `<span>›</span><button data-act="browse-dir" data-source="${esc(b.source_id)}" data-path="${esc(cumulative)}" type="button">${esc(part)}</button>`;
    }
    const pairing = !!state.pairing;
    const rows = (b.entries || []).map((entry) => {
      const name = esc(entry.name);
      const action = entry.kind === 'directory'
        ? `<button data-act="browse-dir" data-source="${esc(b.source_id)}" data-path="${esc(entry.relative_path)}" type="button">打开</button>`
        : pairing
          ? `<button class="primary" data-act="pair-file" data-source="${esc(b.source_id)}" data-relative="${esc(entry.relative_path)}" type="button">配对到此层</button>`
          : `<button class="primary" data-act="add-file" data-source="${esc(b.source_id)}" data-relative="${esc(entry.relative_path)}" data-name="${esc(entry.name)}" data-size="${entry.size || 0}" type="button">添加图层</button>`;
      const size = entry.kind === 'directory' ? '—' : formatBytes(entry.size || 0);
      const modified = entry.modified_ms ? fmtTime(entry.modified_ms) : '—';
      return `<tr>
        <td class="name">${entry.kind === 'directory' ? '📁 ' : ''}<strong>${name}</strong><div class="meta">${esc(entry.format || entry.kind)} · <span class="mono">${esc(entry.relative_path)}</span></div></td>
        <td>${size}</td>
        <td class="meta">${modified}</td>
        <td>${action}</td>
      </tr>`;
    }).join('');
    const totalPages = Math.max(1, Math.ceil((b.total || 0) / (b.page_size || 50)));
    const head = `
      <div class="browse-head">
        <div class="row"><strong>${esc(source ? source.display_name : b.source_id)}</strong>${source ? badge(source.scan_status) : ''}</div>
        ${source ? `<div class="meta mono">${esc(source.root_path)}</div>` : ''}
        <div class="meta">${source && source.last_scan_at_ms ? `上次扫描:${fmtTime(source.last_scan_at_ms)}` : '尚未扫描'}
          · 当前目录共 ${b.total || 0} 项${b.truncated ? '(已截断)' : ''}${source && !source.enabled ? ' · 该数据源已停用' : ''}</div>
      </div>`;
    openFloatWindow('浏览数据文件', `${state.pairing ? `<div class="row" style="margin-bottom:6px"><span class="badge warn">正在为图层「${esc(state.pairing.layer)}」选择配对文件</span></div>` : ''}${head}
      <div class="breadcrumb">${crumbs}</div>
      <table class="table browse-table"><thead><tr><th>名称 / 路径</th><th>大小</th><th>修改时间</th><th></th></tr></thead>
      <tbody>${rows || '<tr><td colspan="4" class="meta">该目录没有受支持的数据文件。</td></tr>'}</tbody></table>
      <div class="row end"><button data-act="browse-page" data-source="${esc(b.source_id)}" data-path="${esc(b.path)}" data-page="${Math.max(0, (b.page || 0) - 1)}" ${b.page ? '' : 'disabled'} type="button">上一页</button>
      <span class="meta">第 ${(b.page || 0) + 1} / ${totalPages} 页</span><button data-act="browse-page" data-source="${esc(b.source_id)}" data-path="${esc(b.path)}" data-page="${(b.page || 0) + 1}" ${b.page + 1 < totalPages ? '' : 'disabled'} type="button">下一页</button></div>`);
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
      <div class="row" style="margin-top:5px"><span class="badge ${d.password_required ? 'warn' : 'ok'}">${d.password_required ? '需要密码' : '密码已保存'}</span>
      <button data-act="db-names" data-id="${esc(d.id)}" type="button">浏览数据库</button>
      <button class="danger" data-act="db-remove" data-id="${esc(d.id)}" type="button">移除</button></div>
      <div class="row" style="margin-top:5px"><input type="password" autocomplete="off" data-password="${esc(d.id)}" placeholder="${d.password_required ? '密码(未保存,每次刷新需重新输入)' : '临时密码(仅本次会话,覆盖已保存)'}"><button data-act="db-password" data-id="${esc(d.id)}" type="button">验证并使用</button></div></div>`).join('');
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
        try {
          await api(`/api/sources/databases/${enc(id)}/password`, { method: 'POST', body: { password } });
          if (input) input.value = '';
          await refreshDatabases();
          toast('密码验证通过,已生效');
        } catch (e) {
          // The server tests the password before accepting it; keep the input so the user
          // can correct a typo and submit again.
          toast(`密码未通过验证:${errorText(e)}。可修改后重新提交。`, true);
        }
      } else if (act === 'db-remove') {
        if (!window.confirm('移除该数据库连接?不会删除任何数据。')) return;
        await api(`/api/sources/databases/${enc(id)}`, { method: 'DELETE' });
        state.catalog = null; state.dbNames = null; state.dbBrowse = null;
        closeFloatWindow();
        await refreshDatabases();
      } else if (act === 'db-names') {
        const data = await api(`/api/sources/databases/${enc(id)}/databases`);
        // Reopening the same connection restores its previous database/schema selection.
        const prev = state.dbBrowse && state.dbBrowse.connId === id ? state.dbBrowse : null;
        state.dbBrowse = {
          connId: id,
          databases: data.databases || [],
          selectedDb: prev ? prev.selectedDb : null,
          catalog: null,
          selectedSchema: prev ? prev.selectedSchema : null,
        };
        if (state.dbBrowse.selectedDb) {
          try {
            state.dbBrowse.catalog = await api(`/api/sources/databases/${enc(id)}/catalog?database=${enc(state.dbBrowse.selectedDb)}`);
          } catch {
            state.dbBrowse.selectedDb = null;
            state.dbBrowse.selectedSchema = null;
          }
        }
        renderDatabaseBrowser();
      } else if (act === 'db-refresh') {
        if (!state.dbBrowse || state.dbBrowse.connId !== id) return;
        const data = await api(`/api/sources/databases/${enc(id)}/databases`);
        state.dbBrowse.databases = data.databases || [];
        if (state.dbBrowse.selectedDb) {
          const catalog = await api(`/api/sources/databases/${enc(id)}/catalog?database=${enc(state.dbBrowse.selectedDb)}`);
          state.dbBrowse.catalog = catalog;
          const names = (catalog.schemas || []).map((s) => s.name);
          if (state.dbBrowse.selectedSchema && !names.includes(state.dbBrowse.selectedSchema)) {
            state.dbBrowse.selectedSchema = null;
          }
        }
        renderDatabaseBrowser();
        toast('目录已刷新');
      } else if (act === 'db-open-db') {
        const db = button.dataset.database;
        const data = await api(`/api/sources/databases/${enc(id)}/catalog?database=${enc(db)}`);
        state.dbBrowse = state.dbBrowse || { connId: id, databases: [db] };
        state.dbBrowse.selectedDb = db;
        state.dbBrowse.catalog = data;
        state.dbBrowse.selectedSchema = null;
        renderDatabaseBrowser();
      } else if (act === 'db-pick-schema') {
        if (state.dbBrowse) {
          state.dbBrowse.selectedSchema = button.dataset.schema;
          renderDatabaseBrowser();
        }
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
  // Database browsing renders in the same floating window as the source browser: a three-pane
  // cascade (databases -> schemas -> tables) read from left to right; a table click adds it.
  function renderDatabaseBrowser() {
    const b = state.dbBrowse;
    if (!b) { return; }
    const conn = (state.databaseViews || []).find((d) => d.id === b.connId);
    const dbItems = (b.databases || []).map((name) =>
      `<button class="db-item${name === b.selectedDb ? ' sel' : ''}" data-act="db-open-db" data-id="${esc(b.connId)}" data-database="${esc(name)}" type="button"><span class="grow">${esc(name)}</span></button>`).join('');
    const schemas = (b.catalog && b.catalog.schemas) || [];
    const schemaItems = b.catalog
      ? schemas.map((s) => `<button class="db-item${s.name === b.selectedSchema ? ' sel' : ''}" data-act="db-pick-schema" data-schema="${esc(s.name)}" type="button"><span class="grow">${esc(s.name)}</span></button>`).join('')
      : '';
    let tableHtml = '';
    if (b.catalog && b.selectedSchema) {
      const schema = schemas.find((s) => s.name === b.selectedSchema);
      const rels = (schema && schema.relations) || [];
      const pairing = !!state.pairing;
      tableHtml = rels.map((rel) => {
        const base = rel.schema === 'public' ? rel.name : `${rel.schema}.${rel.name}`;
        const cols = (rel.geometry_columns || []).map((col) => {
          const table = col.several ? `${base}:${col.name}` : base;
          const act = pairing ? 'pair-db' : 'db-snapshot';
          const label = pairing ? '配对到此层' : `+ 加入地图 · ${esc(col.name)} · ${esc(col.kind)}${col.srid ? ` · EPSG:${esc(col.srid)}` : ''}`;
          return `<button class="primary" data-act="${act}" data-id="${esc(b.connId)}" data-database="${esc(b.catalog.database)}" data-table="${esc(table)}" data-name="${esc(base)}" type="button">${label}</button>`;
        }).join('');
        return `<div class="rel-name">${esc(rel.name)}<span class="badge">${esc(rel.kind)}</span></div>
          <div class="rel-cols">${cols || '<span class="meta">无几何列</span>'}</div>`;
      }).join('');
      if (!tableHtml) tableHtml = '<div class="db-empty">该 schema 下没有表或视图。</div>';
    } else if (b.catalog) {
      tableHtml = '<div class="db-empty">点击中间栏的 schema 查看表。</div>';
    } else {
      tableHtml = '<div class="db-empty">点击左侧数据库加载 schema。</div>';
    }
    openFloatWindow(`数据库目录 · ${conn ? conn.display_name : b.connId}`, `<div class="db-wrap">
      <div class="row db-note"><span class="grow">${state.pairing ? `<span class="badge warn">正在为图层「${esc(state.pairing.layer)}」选择配对表</span> ` : ''}数据库 → schema → 表,点「+ 加入地图」添加快照图层。${b.catalog ? (b.catalog.postgis_available ? '已检测到 PostGIS。' : '未检测到 PostGIS 扩展,几何图层不可用。') : ''}</span>
      <button data-act="db-refresh" data-id="${esc(b.connId)}" type="button">⟳ 刷新目录</button></div>
      <div class="db3">
        <div class="db-pane p1"><h4>数据库</h4>${dbItems || '<div class="db-empty">没有可访问的数据库。</div>'}</div>
        <div class="db-div" data-div="schema" title="左右拖动改变数据库栏宽度，Schema 栏同步增减；双击恢复默认"></div>
        <div class="db-pane p2" style="width:${savedPaneWidth(DB_SCHEMA_WIDTH_KEY)}px"><h4>Schema</h4>${schemaItems || `<div class="db-empty">${b.catalog ? '该数据库没有 schema。' : '先选择左侧数据库。'}</div>`}</div>
        <div class="db-div" data-div="table" title="左右拖动改变 Schema 栏宽度，表栏同步增减；双击恢复默认"></div>
        <div class="db-pane p3" style="width:${savedPaneWidth(DB_TABLE_WIDTH_KEY)}px"><h4>表 / 视图</h4>${tableHtml}</div>
      </div></div>`);
    const db3 = document.querySelector('#float-win .db3');
    if (!db3) return;
    fitPaneWidths(db3, savedPaneWidth(DB_SCHEMA_WIDTH_KEY), savedPaneWidth(DB_TABLE_WIDTH_KEY));
    for (const div of db3.querySelectorAll('.db-div')) bindPaneDivider(div);
    watchPaneWidths(db3);
  }

  // ---- QGIS import --------------------------------------------------------
  async function importQgisFile(file) {
    try {
      const bytes = new Uint8Array(await file.arrayBuffer());
      const data = await api(`/api/import/qgis?filename=${enc(file.name)}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/octet-stream' },
        body: bytes,
        raw: true,
      });
      state.importPlan = { token: data.token, title: data.title || '导入工程', file: file.name, layers: data.layers || [] };
      renderImportPlan();
    } catch (e) {
      toast(`解析 QGIS 工程失败:${errorText(e)}`, true);
    }
  }
  function renderImportPlan() {
    const plan = state.importPlan;
    if (!plan) { return; }
    const rows = plan.layers.map((l) => {
      const status = l.status === 'paired'
        ? '<span class="badge ok">已配对</span>'
        : l.status === 'candidate' ? '<span class="badge warn">可配对</span>' : '<span class="badge bad">未配对</span>';
      const style = l.style === '完整'
        ? '<span class="badge ok">样式完整</span>'
        : l.style === '近似' ? '<span class="badge warn">样式近似</span>' : '<span class="badge">样式未移植</span>';
      return `<tr><td class="name"><strong>${esc(l.name)}</strong></td><td>${esc(l.kind)}</td>
        <td>${status}<div class="meta">${esc(l.detail || '')}</div></td><td>${style}</td></tr>`;
    }).join('');
    const paired = plan.layers.filter((l) => l.status === 'paired').length;
    const unpaired = plan.layers.filter((l) => l.status === 'unpaired').length;
    openFloatWindow('导入 QGIS 工程', `<div class="browse-head">
        <div class="row"><strong>${esc(plan.file)}</strong></div>
        <div class="meta">共 ${plan.layers.length} 个图层:${paired} 个已配对数据,${unpaired} 个待配对(导入后显示为不可用,数据就位后可恢复)。</div>
      </div>
      <table class="table browse-table"><thead><tr><th>图层</th><th>类型</th><th>数据状态</th><th>样式</th></tr></thead>
      <tbody>${rows || '<tr><td colspan="4" class="meta">工程中没有可导入的图层。</td></tr>'}</tbody></table>
      <div class="row" style="margin-top:10px">
        <input id="import-qgis-name" value="${esc(plan.title)}" placeholder="工程名称" style="flex:1">
        <button class="primary" data-act="import-qgis-create" type="button">导入为工程</button>
      </div>`);
  }
  async function confirmQgisImport() {
    const plan = state.importPlan;
    if (!plan) return;
    const name = ($('import-qgis-name') && $('import-qgis-name').value.trim()) || plan.title || '导入工程';
    try {
      const created = await api('/api/import/qgis/create', { method: 'POST', body: { token: plan.token, name } });
      state.importPlan = null;
      closeFloatWindow();
      const w = await api(`/api/projects/${enc(created.project.id)}/copies`, { method: 'POST', body: { versionId: created.version.id } });
      await refreshProjects();
      await openWorking(created.project.id, w.id, { skipConfirm: true });
      toast('QGIS 工程已导入;未配对图层保留在工程中,数据准备好后可恢复显示');
    } catch (e) {
      toast(`导入失败:${errorText(e)}`, true);
    }
  }

  // ---- event wiring -------------------------------------------------------
  // Drag the divider to resize the left panel (280-450px), remembered across reloads.
  const SIDEBAR_MIN = 280;
  const SIDEBAR_MAX = 450;
  function applySidebarWidth(px) {
    const width = Math.min(SIDEBAR_MAX, Math.max(SIDEBAR_MIN, Math.round(px)));
    document.querySelector('.app').style.setProperty('--sidebar-w', `${width}px`);
    try { localStorage.setItem('rustgis-sidebar-w', String(width)); } catch { /* storage may be blocked */ }
    return width;
  }
  const savedWidth = Number(localStorage.getItem('rustgis-sidebar-w'));
  if (savedWidth >= SIDEBAR_MIN && savedWidth <= SIDEBAR_MAX) applySidebarWidth(savedWidth);
  const handle = $('sidebar-resize');
  handle.addEventListener('pointerdown', (e) => {
    e.preventDefault();
    handle.setPointerCapture(e.pointerId);
    handle.classList.add('dragging');
    document.body.style.userSelect = 'none';
    const onMove = (ev) => { applySidebarWidth(ev.clientX); };
    const onUp = (ev) => {
      handle.releasePointerCapture(ev.pointerId);
      handle.classList.remove('dragging');
      document.body.style.userSelect = '';
      handle.removeEventListener('pointermove', onMove);
      handle.removeEventListener('pointerup', onUp);
    };
    handle.addEventListener('pointermove', onMove);
    handle.addEventListener('pointerup', onUp);
  });

  // Collapse the whole left panel (projects/sources/databases) and remember the choice.
  function applySidebar(collapsed) {
    document.querySelector('.app').classList.toggle('no-sidebar', collapsed);
    $('toggle-sidebar').textContent = collapsed ? '展开面板' : '收起面板';
    try { localStorage.setItem('rustgis-sidebar-collapsed', collapsed ? '1' : '0'); } catch { /* storage may be blocked */ }
  }
  applySidebar(localStorage.getItem('rustgis-sidebar-collapsed') === '1');
  $('toggle-sidebar').addEventListener('click', () => {
    applySidebar(!document.querySelector('.app').classList.contains('no-sidebar'));
  });

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
  $('pair-layers').addEventListener('click', renderPairingWindow);
  // Editing tools live in the app bar; they drive the map through the embed protocol.
  $('map-undo').addEventListener('click', () => runCommand('edit.undo').catch((e) => toast(`撤销失败:${errorText(e)}`, true)));
  $('map-redo').addEventListener('click', () => runCommand('edit.redo').catch((e) => toast(`重做失败:${errorText(e)}`, true)));
  $('map-search').addEventListener('click', () => runCommand('ui.commandSearch').catch((e) => toast(`无法打开命令搜索:${errorText(e)}`, true)));
  $('add-source').addEventListener('click', addSource);
  $('add-database').addEventListener('click', addDatabase);
  $('import-qgis').addEventListener('click', () => $('import-qgis-file').click());
  $('import-qgis-file').addEventListener('change', (e) => {
    const file = e.target.files && e.target.files[0];
    e.target.value = '';
    if (file) importQgisFile(file);
  });

  $('project-list').addEventListener('click', (e) => {
    const b = e.target.closest('button[data-act]');
    if (!b) return;
    if (b.dataset.act === 'open-version') openVersion(b.dataset.project, b.dataset.version);
    else if (b.dataset.act === 'copy-version') copyVersionDialog(b.dataset.project, b.dataset.version);
    else if (b.dataset.act === 'open-working') openWorking(b.dataset.project, b.dataset.working);
    else if (b.dataset.act === 'edit-project') editProjectInfo(b.dataset.project);
    else if (b.dataset.act === 'toggle-project') {
      const id = b.dataset.project;
      if (state.openProjects.has(id)) state.openProjects.delete(id);
      else state.openProjects.add(id);
      const group = b.closest('.project-card');
      group.classList.toggle('collapsed');
      const collapsed = group.classList.contains('collapsed');
      b.textContent = collapsed ? '▶' : '▼';
      b.setAttribute('aria-expanded', collapsed ? 'false' : 'true');
    }
  });
  $('project-detail').addEventListener('click', (e) => {
    const b = e.target.closest('button[data-act]');
    if (b && b.dataset.act === 'open-working') openWorking(b.dataset.project, b.dataset.working);
  });
  $('source-list').addEventListener('click', (e) => {
    const b = e.target.closest('button[data-act]');
    if (b) sourceAction(b);
  });
  // The source browser and database browser render inside the floating window; their
  // actions are delegated here.
  $('float-win').addEventListener('click', (e) => {
    const b = e.target.closest('button[data-act]');
    if (!b) return;
    if (b.dataset.act === 'browse-dir') loadBrowse(b.dataset.source, b.dataset.path || '', 0);
    else if (b.dataset.act === 'browse-page') loadBrowse(b.dataset.source, b.dataset.path || '', Number(b.dataset.page || 0));
    else if (b.dataset.act === 'add-file') addFileLayer(b.dataset.source, b.dataset.relative, b.dataset.name, b.dataset.size);
    else if (b.dataset.act === 'db-open-db' || b.dataset.act === 'db-pick-schema' || b.dataset.act === 'db-snapshot' || b.dataset.act === 'db-refresh') databaseAction(b);
    else if (b.dataset.act === 'import-qgis-create') confirmQgisImport();
    else if (b.dataset.act === 'pair-file-pick') {
      state.pairing = { layer: b.dataset.layer };
      const source = state.sources.find((s) => s.enabled);
      if (!source) { toast('请先在「数据源」里配置一个目录', true); state.pairing = null; return; }
      loadBrowse(source.id, (state.browsePaths && state.browsePaths[source.id]) || '', 0);
    }
    else if (b.dataset.act === 'pair-db-pick') {
      state.pairing = { layer: b.dataset.layer };
      const views = state.databaseViews || [];
      const target = b.dataset.target || '';
      const conn = views.find((d) => target.includes(d.display_name)) || views[0];
      if (!conn) { toast('请先在「数据库」里添加连接', true); state.pairing = null; return; }
      openDbBrowse(conn.id);
    }
    else if (b.dataset.act === 'pair-file') doPairFile(b.dataset.source, b.dataset.relative);
    else if (b.dataset.act === 'pair-db') doPairDb(b.dataset.id, b.dataset.database, b.dataset.table);
  });
  $('database-list').addEventListener('click', (e) => {
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
