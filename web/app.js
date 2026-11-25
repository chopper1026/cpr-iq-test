'use strict';
(() => {
  const $ = id => document.getElementById(id);
  const bridge = window.codexProxyPlugin;

  const reasons = {
    invalidInput: '请检查所选 Client Key、模型、推理档位与账号配置是否有效。',
    conflict: '已有相同账号与模型档位在队列中，或巡检设置已被更新，请刷新后重试。',
    queueFull: '待执行队列已满，请等待当前任务完成或取消部分排队任务后再试。',
    budget: '旧版预算限制跳过。',
    unsupportedProfile: '宿主或上游不支持该模型或推理档位。',
    detectorUnavailable: '本地指纹参考库未就绪，未发送探针。',
    accountUnavailable: '指定账号不可用或不再是启用的 OAuth 账号。',
    accountChanged: '采样期间上游账号身份发生变化（账号被替换）。',
    accessDenied: '所选 Client Key 无法访问所选账号或该模型。',
    rateLimited: '宿主或上游触发限流。',
    timeout: '模型调用或宿主回调超时。',
    hostFailure: '宿主未能完成本次操作。',
    cancelled: '任务已取消。',
    incomplete: '模型回复未完整结束、被截断或不可用。',
    transformed: '回复被宿主中间件改写，未参与评分。',
    notScorable: '样本不可评分，有效数字不足或不符合语言模型分布。',
    nearby: '目标模型名次接近或头两名差距过小，无法单独判定。',
    inconsistent: '两轮独立检测结果不一致。',
    versionChanged: '复测前模型、参考库或上游账号身份发生变化。',
    restarted: '上次调用期间插件或宿主重启；为避免重复消耗，未自动重发。',
    scheduleChanged: '定时巡检已关闭或计划已变更。',
    staleSlot: '旧巡检周期已过期，已跳过。',
  };

  const verdictLabels = {
    match: '指纹匹配',
    ranked: '参考排名',
    suspect: '疑似指纹异常',
    insufficient: '证据不足',
    failed: '检测失败',
    interrupted: '已中断',
    skipped: '已跳过',
    running: '检测中',
    queued: '等待检测',
  };

  const effortLabels = {
    default: '默认（不指定）',
    none: 'none（关闭）',
    minimal: 'minimal',
    low: 'low',
    medium: 'medium',
    high: 'high',
    xhigh: 'xhigh',
    max: 'max',
    ultra: 'ultra',
  };

  const stamp = new Intl.DateTimeFormat('zh-CN', { dateStyle: 'medium', timeStyle: 'short', hour12: false });
  const localDate = at => at == null ? '—' : stamp.format(new Date(at));
  const reasonText = code => reasons[code] || '请求失败，请刷新后重试。';
  const effortLabel = effort => effortLabels[effort] || effort || '默认';

  const make = (tag, text, className) => {
    const el = document.createElement(tag);
    if (text != null) el.textContent = text;
    if (className) el.className = className;
    return el;
  };
  const option = (value, text) => {
    const item = make('option', text);
    item.value = value;
    return item;
  };

  function status(text, error = false) {
    $('accessible-status').textContent = text;
    $('accessible-status').classList.toggle('error', error);
  }

  if (!bridge || typeof bridge.request !== 'function') {
    status('请从 Codex Proxy 的已启用插件实例打开此页面。', true);
    $('refresh').disabled = true;
    return;
  }

  function applyTheme() {
    document.documentElement.dataset.theme = bridge.theme === 'dark' ? 'dark' : 'light';
  }
  window.addEventListener('codex-proxy-themechange', applyTheme);
  applyTheme();

  async function request(method, path, body, queryParams) {
    let query = '';
    if (queryParams) {
      const sp = new URLSearchParams();
      for (const [k, v] of Object.entries(queryParams)) {
        if (v !== '' && v != null) sp.set(k, String(v));
      }
      query = sp.toString();
    }
    const payload = { method, path };
    if (query) payload.query = query;
    if (body != null) {
      payload.contentType = 'application/json';
      payload.body = JSON.stringify(body);
    }
    const res = await bridge.request(payload);
    let json;
    try {
      json = JSON.parse(new TextDecoder().decode(res.body));
    } catch {
      throw new Error('hostFailure');
    }
    if (res.status < 200 || res.status >= 300) {
      throw new Error(json.error || 'hostFailure');
    }
    return json;
  }

  let catalog = { accounts: [], keys: [], models: [], maxBatch: 100, detectorReady: false };
  let schedule = null;
  let runsData = null;
  let activeTab = 'detect';
  let busy = false;
  let polling = false;
  let nonce = null;
  let searchTimer = null;
  let logBundle = null;
  let logTargetRunId = null;
  const canDownloadFile = window.origin !== 'null';

  const queryState = {
    page: 1,
    pageSize: 20,
    search: '',
    source: '',
    verdict: '',
    model: '',
    effort: '',
  };

  function setTab(name) {
    activeTab = name;
    const isDetect = name === 'detect';
    $('tab-detect').classList.toggle('active', isDetect);
    $('tab-detect').setAttribute('aria-selected', String(isDetect));
    $('panel-detect').hidden = !isDetect;
    $('tab-records').classList.toggle('active', !isDetect);
    $('tab-records').setAttribute('aria-selected', String(!isDetect));
    $('panel-records').hidden = isDetect;
    if (!isDetect) {
      fetchRuns();
    }
  }

  $('tab-detect').addEventListener('click', () => setTab('detect'));
  $('tab-records').addEventListener('click', () => setTab('records'));
  $('goto-records').addEventListener('click', () => setTab('records'));
  $('summary-records-card').addEventListener('click', () => setTab('records'));
  $('summary-records-card').addEventListener('keydown', e => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      setTab('records');
    }
  });

  function formatAccountPrimary(account) {
    if (account.email) return account.email;
    if (account.name && account.name !== account.id) return account.name;
    return account.id;
  }

  function createTargetControls(containerId) {
    const key = make('select');
    const model = make('select');
    const effort = make('select');
    const search = make('input', null, 'account-search');
    search.type = 'search';
    search.placeholder = '搜索账号邮箱、名称或 ID…';
    const accounts = make('div', null, 'accounts');
    const selectAllBtn = make('button', '全选', 'mini-btn');
    selectAllBtn.type = 'button';
    const clearBtn = make('button', '清空', 'mini-btn');
    clearBtn.type = 'button';

    const keyLabel = make('label', '测试 Client Key');
    keyLabel.append(key);

    const modelLabel = make('label', '检测模型');
    modelLabel.append(model);
    const effortLabelEl = make('label', '推理档位');
    effortLabelEl.append(effort);
    const modelRow = make('div', null, 'fields');
    modelRow.append(modelLabel, effortLabelEl);

    const legend = make('legend', '指定账号');
    const tools = make('div', null, 'account-tools');
    tools.append(selectAllBtn, clearBtn);
    const head = make('div', null, 'account-head');
    head.append(legend, tools);

    const group = make('fieldset');
    group.append(head, search, accounts);
    $(containerId).append(keyLabel, modelRow, group);

    const selectedIds = new Set();
    const ctrl = { key, model, effort, search, accounts, legend, selectAllBtn, clearBtn, selectedIds };

    model.addEventListener('change', () => {
      syncEfforts(ctrl);
      updateButtons();
    });
    search.addEventListener('input', () => {
      renderAccountList(ctrl);
    });
    selectAllBtn.addEventListener('click', () => {
      const visible = filteredAccounts(search.value);
      for (const a of visible) {
        if (selectedIds.size >= catalog.maxBatch) break;
        selectedIds.add(a.id);
      }
      renderAccountList(ctrl);
      updateButtons();
    });
    clearBtn.addEventListener('click', () => {
      selectedIds.clear();
      renderAccountList(ctrl);
      updateButtons();
    });

    return ctrl;
  }

  const manual = createTargetControls('manual-target');
  const planned = createTargetControls('schedule-target');

  function filteredAccounts(keyword) {
    const q = (keyword || '').trim().toLowerCase();
    if (!q) return catalog.accounts;
    return catalog.accounts.filter(a =>
      (a.email && a.email.toLowerCase().includes(q)) ||
      (a.name && a.name.toLowerCase().includes(q)) ||
      a.id.toLowerCase().includes(q) ||
      (a.planType && a.planType.toLowerCase().includes(q))
    );
  }

  function renderAccountList(ctrl) {
    const visible = filteredAccounts(ctrl.search.value);
    const atLimit = ctrl.selectedIds.size >= catalog.maxBatch;
    if (!catalog.accounts.length) {
      ctrl.accounts.replaceChildren(make('div', '暂无启用的 OpenAI OAuth 账号', 'account muted'));
    } else if (!visible.length) {
      ctrl.accounts.replaceChildren(make('div', '没有匹配该关键字的账号', 'account muted'));
    } else {
      ctrl.accounts.replaceChildren(...visible.map(a => {
        const check = make('input');
        check.type = 'checkbox';
        check.value = a.id;
        check.checked = ctrl.selectedIds.has(a.id);
        check.disabled = !check.checked && atLimit;
        check.addEventListener('change', () => {
          if (check.checked) ctrl.selectedIds.add(a.id);
          else ctrl.selectedIds.delete(a.id);
          renderAccountList(ctrl);
          updateButtons();
        });

        const primary = make('div', null, 'account-primary');
        primary.append(make('span', formatAccountPrimary(a)));
        if (a.name && a.name !== a.id && a.name !== a.email) {
          primary.append(make('span', a.name, 'alias-pill'));
        }
        if (a.planType) {
          primary.append(make('span', a.planType, 'plan-pill'));
        }

        const info = make('div', null, 'account-info');
        info.append(primary);
        if (a.email || (a.name && a.name !== a.id)) {
          info.append(make('div', a.id, 'account-sub'));
        }

        const row = make('label', null, 'account');
        row.title = a.id;
        row.append(check, info);
        return row;
      }));
    }
    const total = catalog.accounts.length;
    const count = ctrl.selectedIds.size;
    ctrl.legend.textContent = `指定账号（已选 ${count} / 共 ${total}）`;
    ctrl.selectAllBtn.disabled = !visible.length || count >= catalog.maxBatch;
    ctrl.clearBtn.disabled = count === 0;
  }

  function syncEfforts(ctrl, preferredEffort) {
    const currentEffort = preferredEffort != null ? preferredEffort : ctrl.effort.value;
    const selectedModel = catalog.models.find(m => m.id === ctrl.model.value);
    const efforts = selectedModel?.efforts?.length
      ? selectedModel.efforts
      : (currentEffort ? [currentEffort] : []);
    if (!efforts.length) {
      ctrl.effort.replaceChildren(option('', '请先选择模型'));
      ctrl.effort.value = '';
      return;
    }
    ctrl.effort.replaceChildren(...efforts.map(e => option(e, effortLabel(e))));
    if (efforts.includes(currentEffort)) {
      ctrl.effort.value = currentEffort;
    } else if (selectedModel && efforts.includes(selectedModel.defaultEffort)) {
      ctrl.effort.value = selectedModel.defaultEffort;
    } else {
      ctrl.effort.value = efforts[0];
    }
  }

  function readTarget(ctrl) {
    return {
      clientKeyId: ctrl.key.value,
      model: ctrl.model.value,
      effort: ctrl.effort.value,
      accountIds: [...ctrl.selectedIds].sort(),
    };
  }

  function isTargetUsable(t) {
    return Boolean(t.clientKeyId && t.model && t.effort && t.accountIds.length > 0 && t.accountIds.length <= catalog.maxBatch);
  }

  function applyTarget(ctrl, chosen) {
    const keyOpts = [option('', catalog.keys.length ? '选择已启用 Client Key' : '暂无已启用 Client Key'),
      ...catalog.keys.map(k => option(k.id, k.name ? `${k.name} (${k.id})` : k.id))];
    ctrl.key.replaceChildren(...keyOpts);
    const desiredKey = chosen.clientKeyId || (catalog.keys.length === 1 ? catalog.keys[0].id : '');
    ctrl.key.value = catalog.keys.some(k => k.id === desiredKey) ? desiredKey : '';

    const modelList = [...catalog.models];
    if (chosen.model && !modelList.some(m => m.id === chosen.model)) {
      modelList.push({
        id: chosen.model,
        displayName: chosen.model,
        efforts: [chosen.effort || 'default'],
        defaultEffort: chosen.effort || 'default',
        inReferenceBank: false,
      });
    }
    const modelOpts = [
      option('', modelList.length ? '选择模型' : '暂未获取到模型目录'),
      ...modelList.map(m => {
        const suffix = m.inReferenceBank ? ' · 参考库' : '';
        const title = m.displayName && m.displayName !== m.id ? `${m.id} (${m.displayName})${suffix}` : `${m.id}${suffix}`;
        return option(m.id, title);
      }),
    ];
    ctrl.model.replaceChildren(...modelOpts);
    const desiredModel = chosen.model || (modelList.length ? modelList[0].id : '');
    ctrl.model.value = modelList.some(m => m.id === desiredModel) ? desiredModel : '';
    syncEfforts(ctrl, chosen.effort);

    ctrl.selectedIds.clear();
    const validAccountIds = new Set(catalog.accounts.map(a => a.id));
    for (const id of chosen.accountIds || []) {
      if (validAccountIds.has(id)) ctrl.selectedIds.add(id);
    }
    renderAccountList(ctrl);
  }

  function describeCost(t) {
    const count = t.accountIds.length;
    if (!count) return '请先选择至少 1 个账号';
    const modelMeta = catalog.models.find(m => m.id === t.model);
    const base = count * 3;
    const range = modelMeta?.inReferenceBank ? `${base}–${base * 2}` : `${base}`;
    return `${count} 个账号，预计 ${range} 次模型调用（按宿主 Client Key 规则计费）`;
  }

  function updateButtons() {
    const m = readTarget(manual);
    const s = readTarget(planned);
    $('manual-cost').textContent = `预计调用：${describeCost(m)}`;
    $('schedule-cost').textContent = $('enabled').checked
      ? `每次巡检：${describeCost(s)}`
      : '定时巡检已关闭，不会自动发起调用。';
    $('submit-run').disabled = busy || !catalog.detectorReady || !isTargetUsable(m);
    $('save').disabled = busy || !schedule || ($('enabled').checked && (!catalog.detectorReady || !isTargetUsable(s)));
  }

  function applySchedule(data) {
    schedule = data;
    $('enabled').checked = Boolean(data.settings.enabled);
    $('hours').value = String(data.settings.hours || 24);
    applyTarget(planned, data.settings.target || { clientKeyId: '', model: '', effort: '', accountIds: [] });
    $('schedule-notice').textContent = data.lastSkip ? `上次巡检跳过原因：${reasonText(data.lastSkip)}` : '';
    renderSummary();
    updateButtons();
  }

  function renderSummary() {
    const sum = runsData?.summary;
    if (sum) {
      const active = sum.running + sum.queued;
      if (sum.running > 0) {
        $('queue-count').textContent = `${sum.running} 执行中 · ${sum.queued} 等待中`;
      } else if (sum.queued > 0) {
        $('queue-count').textContent = `${sum.queued} 项等待执行`;
      } else {
        $('queue-count').textContent = '空闲';
      }
      $('history-summary').textContent = `共 ${sum.total} 条（匹配 ${sum.match} · 异常 ${sum.suspect}）`;
      $('tab-records-badge').textContent = String(sum.total);
      $('tab-records-badge').classList.toggle('active-work', active > 0);
      $('cancel-pending').hidden = sum.queued === 0;
      $('cancel-pending').textContent = `取消待执行（${sum.queued}）`;
    } else {
      $('queue-count').textContent = '—';
      $('history-summary').textContent = '—';
    }
    if (schedule) {
      if (schedule.settings.enabled) {
        $('due').textContent = `每 ${schedule.settings.hours} 小时 · 下次 ${localDate(schedule.nextDueAt)}`;
      } else {
        $('due').textContent = '已关闭';
      }
      $('last-skip').hidden = !schedule.lastSkip;
      $('last-skip').textContent = schedule.lastSkip ? `上次定时巡检未入队：${reasonText(schedule.lastSkip)}` : '';
    }
  }

  function renderQuickPills(summary) {
    if (!summary) return;
    const map = {
      all: summary.total,
      running: summary.running,
      queued: summary.queued,
      match: summary.match,
      ranked: summary.ranked,
      suspect: summary.suspect,
      insufficient: summary.insufficient,
      failed: summary.failed,
    };
    for (const pill of $('quick-pills').querySelectorAll('.pill')) {
      const verdict = pill.dataset.verdict || '';
      pill.classList.toggle('active', queryState.verdict === verdict);
      const countEl = pill.querySelector('[data-count]');
      if (countEl) {
        const key = countEl.dataset.count;
        countEl.textContent = String(map[key] ?? 0);
      }
    }
  }

  function syncFilterOptions(filters) {
    const modelSet = new Set([...(filters?.models || []), ...catalog.models.map(m => m.id)]);
    if (queryState.model) modelSet.add(queryState.model);
    const models = [...modelSet].sort();
    $('filter-model').replaceChildren(option('', '全部模型'), ...models.map(m => option(m, m)));
    $('filter-model').value = queryState.model;

    const effortSet = new Set(filters?.efforts || []);
    for (const m of catalog.models) {
      for (const e of m.efforts || []) effortSet.add(e);
    }
    if (queryState.effort) effortSet.add(queryState.effort);
    const efforts = [...effortSet];
    $('filter-effort').replaceChildren(option('', '全部档位'), ...efforts.map(e => option(e, effortLabel(e))));
    $('filter-effort').value = queryState.effort;

    const hasFilter = Boolean(
      queryState.search || queryState.source || queryState.verdict || queryState.model || queryState.effort
    );
    $('reset-filters').disabled = !hasFilter;
  }

  function formatProb(bps) {
    if (bps == null) return '';
    return `${(bps / 100).toFixed(1)}%`;
  }

  function renderRankingCell(item) {
    const td = make('td');
    const rounds = [item.firstRound, item.outcome?.lastRound].filter(Boolean);
    if (!rounds.length) {
      td.textContent = '—';
      return td;
    }
    for (const [idx, round] of rounds.entries()) {
      const block = make('div', null, 'rank-round');
      const candidates = round.ranking?.candidates || [];
      if (!candidates.length) continue;
      const prefix = rounds.length > 1 ? `第 ${idx + 1} 轮 · ` : '';
      const first = candidates[0];
      const topLine = make('div', null, 'rank-top');
      topLine.append(document.createTextNode(`${prefix}#1 ${first.label}`));
      if (first.probabilityBps != null) {
        topLine.append(make('span', formatProb(first.probabilityBps), 'rank-prob'));
      }
      topLine.title = `前二差 ${(round.topMarginBps / 100).toFixed(1)} 个百分点 · 参考库 ${round.detectorRevision || round.stamp}`;
      block.append(topLine);

      if (candidates.length > 1) {
        const rest = candidates.slice(1).map((c, i) => {
          const prob = c.probabilityBps != null ? ` ${formatProb(c.probabilityBps)}` : '';
          return `#${i + 2} ${c.label}${prob}`;
        }).join(' · ');
        block.append(make('div', rest, 'rank-others'));
      }
      td.append(block);
    }
    return td;
  }

  function resolveRecordEmail(item) {
    if (item.accountEmail) return item.accountEmail;
    const live = catalog.accounts.find(a => a.id === item.accountId);
    if (live?.email) return live.email;
    if (item.accountName && item.accountName !== item.accountId) return item.accountName;
    if (live?.name && live.name !== live.id) return live.name;
    return item.accountId;
  }

  function renderPagination(pagination) {
    const { page, pageSize, total, totalPages } = pagination;
    $('pagination').hidden = total === 0;
    if (total === 0) return;

    const start = (page - 1) * pageSize + 1;
    const end = Math.min(page * pageSize, total);
    $('page-info').textContent = `共 ${total} 条记录，当前显示第 ${start}–${end} 条（第 ${page} / ${totalPages} 页）`;
    $('page-size').value = String(pageSize);

    const buttons = [];
    const addBtn = (label, targetPage, disabled, current = false) => {
      const btn = make('button', label, current ? 'page-btn current' : 'page-btn');
      btn.type = 'button';
      btn.disabled = disabled;
      if (!disabled && !current) {
        btn.addEventListener('click', () => {
          queryState.page = targetPage;
          fetchRuns();
        });
      }
      buttons.push(btn);
    };

    addBtn('首页', 1, page <= 1);
    addBtn('上一页', page - 1, page <= 1);

    const pages = new Set([1, totalPages]);
    for (let p = Math.max(1, page - 2); p <= Math.min(totalPages, page + 2); p += 1) {
      pages.add(p);
    }
    const sorted = [...pages].sort((a, b) => a - b);
    let prev = 0;
    for (const p of sorted) {
      if (prev && p - prev > 1) {
        buttons.push(make('span', '…', 'page-ellipsis'));
      }
      addBtn(String(p), p, false, p === page);
      prev = p;
    }

    addBtn('下一页', page + 1, page >= totalPages);
    addBtn('末页', totalPages, page >= totalPages);
    $('page-buttons').replaceChildren(...buttons);
  }

  function renderRuns(data) {
    runsData = data;
    renderSummary();
    renderQuickPills(data.summary);
    syncFilterOptions(data.filters);

    const items = data.items || [];
    const rows = items.map(item => {
      const tr = make('tr');

      const accountTd = make('td');
      const primary = resolveRecordEmail(item);
      accountTd.append(make('div', primary, 'cell-main'));
      const meta = make('div', null, 'cell-meta');
      meta.append(make('span', item.scheduled ? '定时巡检' : '手动检测', `source-tag${item.scheduled ? ' scheduled' : ''}`));
      if (item.accountName && item.accountName !== primary && item.accountName !== item.accountId) {
        meta.append(make('span', item.accountName, 'alias-pill'));
      }
      if (primary !== item.accountId) {
        meta.append(make('span', item.accountId, 'mono'));
      }
      accountTd.append(meta);
      accountTd.title = item.accountId;

      const modelTd = make('td');
      modelTd.append(make('span', item.model || '—', 'cell-main'));
      if (item.effort) {
        modelTd.append(make('span', effortLabel(item.effort), 'effort-tag'));
      }

      const statusTd = make('td');
      const code = item.status || item.outcome?.verdict || 'queued';
      statusTd.append(make('span', verdictLabels[code] || code, `badge ${code}`));
      if (item.outcome?.reason) {
        statusTd.append(make('div', reasonText(item.outcome.reason), 'detail'));
      } else if (item.kind === 'queue') {
        statusTd.append(make('div', item.round === 2 ? '第 2 轮复测中' : '第 1 轮检测', 'detail'));
      }
      if (item.diagnostics) {
        const d = item.diagnostics;
        const okProbes = (d.probes || []).filter(p => p.ok).length;
        const totalProbes = (d.probes || []).length;
        const sec = d.durationMs != null ? `${(d.durationMs / 1000).toFixed(1)}s` : '';
        const revNote = d.credentialRevisionBefore != null && d.credentialRevisionAfter != null && d.credentialRevisionBefore !== d.credentialRevisionAfter
          ? ` · 凭据版本 ${d.credentialRevisionBefore}→${d.credentialRevisionAfter}`
          : '';
        const summaryParts = [sec, totalProbes ? `探针 ${okProbes}/${totalProbes}` : ''].filter(Boolean).join(' · ');
        if (summaryParts || revNote) {
          statusTd.append(make('div', `${summaryParts}${revNote}`, 'detail mono'));
        }
        if (d.errorDetail && code !== 'match' && code !== 'ranked') {
          statusTd.append(make('div', d.errorDetail, 'detail mono'));
        }
      }

      const timeTd = make('td');
      timeTd.append(make('div', localDate(item.endedAt || item.queuedAt)));
      timeTd.append(make('div', item.runId, 'detail mono'));
      timeTd.title = `任务 ${item.runId}${item.pluginVersion ? ` · 插件 ${item.pluginVersion}` : ''}`;

      const actionTd = make('td');
      const logBtn = make('button', '日志', 'mini-btn');
      logBtn.type = 'button';
      logBtn.title = `查看并导出任务 ${item.runId} 的诊断日志`;
      logBtn.addEventListener('click', () => openLogs(item.runId));
      actionTd.append(logBtn);

      tr.append(accountTd, modelTd, statusTd, renderRankingCell(item), timeTd, actionTd);
      return tr;
    });

    $('rows').replaceChildren(...rows);
    $('table-wrap').hidden = rows.length === 0;
    $('empty').hidden = rows.length > 0;
    if (!rows.length) {
      const filtered = Boolean(queryState.search || queryState.source || queryState.verdict || queryState.model || queryState.effort);
      $('empty').textContent = filtered ? '没有符合筛选条件的任务记录。' : '还没有检测记录。';
    }
    renderPagination(data.pagination);
  }

  async function fetchRuns() {
    try {
      const data = await request('GET', 'runs', null, {
        page: queryState.page,
        pageSize: queryState.pageSize,
        search: queryState.search,
        source: queryState.source,
        verdict: queryState.verdict,
        model: queryState.model,
        effort: queryState.effort,
      });
      renderRuns(data);
    } catch (error) {
      $('records-notice').textContent = reasonText(error.message);
    }
  }

  async function refresh(full, forceModelRefresh = false) {
    if (polling) return;
    polling = true;
    if (full) $('refresh').disabled = true;
    try {
      const catQuery = {};
      if (forceModelRefresh) catQuery.refresh = '1';
      if (manual.key.value) catQuery.clientKeyId = manual.key.value;

      const [nextCatalog, nextRuns, nextSchedule] = await Promise.all([
        full ? request('GET', 'catalog', null, catQuery) : catalog,
        request('GET', 'runs', null, {
          page: queryState.page,
          pageSize: queryState.pageSize,
          search: queryState.search,
          source: queryState.source,
          verdict: queryState.verdict,
          model: queryState.model,
          effort: queryState.effort,
        }),
        request('GET', 'schedule'),
      ]);

      catalog = nextCatalog;
      if (full) {
        const modelCount = catalog.models.length;
        const accountCount = catalog.accounts.length;
        if (!catalog.detectorReady) {
          status('本地参考指纹库未通过校验，暂不可发起检测。', true);
        } else if (!accountCount) {
          status('当前暂无启用的 OpenAI OAuth 账号。', false);
        } else if (!modelCount) {
          status(`已读取 ${accountCount} 个账号，但暂未拉取到可用模型目录${catalog.modelCatalogError ? `（${catalog.modelCatalogError}）` : ''}，可点击右上角「刷新目录」重试。`, true);
        } else {
          const inBank = catalog.models.filter(m => m.inReferenceBank).length;
          status(`已就绪：${accountCount} 个启用账号，自动获取 ${modelCount} 个模型（其中 ${inBank} 个支持指纹结论判定，其余支持参考排名）。`, false);
        }
        $('version').textContent = `插件 ${catalog.pluginVersion} · 参考库 ${catalog.detectorRevision || '未加载'}`;
        applyTarget(manual, readTarget(manual));
        applySchedule(nextSchedule);
      } else {
        schedule = nextSchedule;
      }
      renderRuns(nextRuns);
      updateButtons();
    } catch (error) {
      if (full) status(reasonText(error.message), true);
    } finally {
      polling = false;
      $('refresh').disabled = false;
    }
  }

  manual.key.addEventListener('change', updateButtons);
  manual.effort.addEventListener('change', updateButtons);
  planned.key.addEventListener('change', updateButtons);
  planned.effort.addEventListener('change', updateButtons);
  $('enabled').addEventListener('change', updateButtons);
  $('hours').addEventListener('change', updateButtons);

  $('refresh').addEventListener('click', () => refresh(true, true));
  $('refresh-records').addEventListener('click', () => {
    $('records-notice').textContent = '';
    fetchRuns();
  });

  $('manual').addEventListener('submit', async e => {
    e.preventDefault();
    if ($('submit-run').disabled) return;
    const selected = readTarget(manual);
    const signature = JSON.stringify(selected);
    if (!nonce || nonce.signature !== signature) {
      const bytes = crypto.getRandomValues(new Uint8Array(16));
      nonce = { signature, runId: `m-${Array.from(bytes, b => b.toString(16).padStart(2, '0')).join('')}` };
    }
    busy = true;
    updateButtons();
    $('manual-notice').textContent = '正在加入执行队列…';
    try {
      const accepted = await request('POST', 'runs', { target: selected, runId: nonce.runId });
      nonce = null;
      $('manual-notice').textContent = `已加入队列（${accepted.queuedCount} 个账号，预计 ${accepted.probesMin === accepted.probesMax ? accepted.probesMin : `${accepted.probesMin}–${accepted.probesMax}`} 次调用），等待后台自动执行。`;
      $('goto-records').hidden = false;
      await refresh(false);
    } catch (error) {
      $('manual-notice').textContent = reasonText(error.message);
    } finally {
      busy = false;
      updateButtons();
    }
  });

  $('schedule').addEventListener('submit', async e => {
    e.preventDefault();
    if ($('save').disabled) return;
    const selected = readTarget(planned);
    const settings = {
      enabled: $('enabled').checked,
      hours: Number($('hours').value),
      target: isTargetUsable(selected) ? selected : schedule.settings.target,
    };
    busy = true;
    updateButtons();
    try {
      applySchedule(await request('PUT', 'schedule', { settings, previous: schedule.settings }));
      $('schedule-notice').textContent = settings.enabled ? '定时巡检设置已保存并启用。' : '定时巡检已保存（当前关闭）。';
      await refresh(false);
    } catch (error) {
      $('schedule-notice').textContent = reasonText(error.message);
    } finally {
      busy = false;
      updateButtons();
    }
  });

  $('cancel-pending').addEventListener('click', async () => {
    busy = true;
    $('cancel-pending').disabled = true;
    try {
      const res = await request('POST', 'runs/cancel', {});
      $('records-notice').textContent = res.cancelled > 0
        ? `已取消 ${res.cancelled} 项待执行任务（正在执行中的任务不受影响）。`
        : '没有可取消的待执行任务。';
      await fetchRuns();
    } catch (error) {
      $('records-notice').textContent = reasonText(error.message);
    } finally {
      busy = false;
      $('cancel-pending').disabled = false;
    }
  });

  $('quick-pills').addEventListener('click', e => {
    const pill = e.target.closest('.pill');
    if (!pill) return;
    queryState.verdict = pill.dataset.verdict || '';
    $('filter-verdict').value = queryState.verdict;
    queryState.page = 1;
    fetchRuns();
  });

  $('filter-search').addEventListener('input', () => {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      queryState.search = $('filter-search').value.trim();
      queryState.page = 1;
      fetchRuns();
    }, 250);
  });
  $('filter-search').addEventListener('keydown', e => {
    if (e.key === 'Enter') {
      clearTimeout(searchTimer);
      queryState.search = $('filter-search').value.trim();
      queryState.page = 1;
      fetchRuns();
    }
  });

  for (const [id, field] of [
    ['filter-source', 'source'],
    ['filter-verdict', 'verdict'],
    ['filter-model', 'model'],
    ['filter-effort', 'effort'],
  ]) {
    $(id).addEventListener('change', () => {
      queryState[field] = $(id).value;
      queryState.page = 1;
      fetchRuns();
    });
  }

  $('page-size').addEventListener('change', () => {
    queryState.pageSize = Number($('page-size').value) || 20;
    queryState.page = 1;
    fetchRuns();
  });

  $('reset-filters').addEventListener('click', () => {
    queryState.search = '';
    queryState.source = '';
    queryState.verdict = '';
    queryState.model = '';
    queryState.effort = '';
    queryState.page = 1;
    $('filter-search').value = '';
    $('filter-source').value = '';
    $('filter-verdict').value = '';
    $('filter-model').value = '';
    $('filter-effort').value = '';
    $('records-notice').textContent = '';
    fetchRuns();
  });

  function formatMs(ms) {
    if (ms == null) return '—';
    return ms >= 1000 ? `${(ms / 1000).toFixed(2)}s (${ms}ms)` : `${ms}ms`;
  }

  function formatLogText(bundle) {
    const lines = [];
    lines.push('=== cpr-iq-test 账号指纹巡检诊断日志 ===');
    lines.push(`导出时间: ${new Date(bundle.exportedAt).toISOString()}`);
    lines.push(`插件版本: ${bundle.pluginVersion || 'unknown'}`);
    lines.push(`参考指纹库: ready=${bundle.detectorReady} revision=${bundle.detectorRevision || '-'}`);
    const env = bundle.environment || {};
    lines.push(`环境摘要: 启用账号=${env.accountCount ?? 0} 模型数=${env.modelCount ?? 0} 目录错误=${env.modelCatalogError || '无'}`);
    lines.push(`任务记录数: ${bundle.recordCount ?? 0}  运行期事件日志数: ${bundle.runtimeLogCount ?? 0}`);
    lines.push('');
    for (const rec of bundle.records || []) {
      const verdict = rec.status || rec.outcome?.verdict || '-';
      const reason = rec.outcome?.reason || '-';
      lines.push(`--- 任务 ${rec.runId} ---`);
      lines.push(`账号: ${rec.accountEmail || rec.accountName || rec.accountId} (${rec.accountId})`);
      lines.push(`目标: ${rec.model} / effort=${rec.effort} / 来源=${rec.scheduled ? '定时' : '手动'}`);
      lines.push(`结论: ${verdict} (${verdictLabels[verdict] || verdict})  原因: ${reason}`);
      if (rec.outcome?.lastRound?.ranking?.candidates?.length) {
        const cands = rec.outcome.lastRound.ranking.candidates
          .map((c, idx) => `#${idx + 1} ${c.label}${c.probabilityBps != null ? ` (${formatProb(c.probabilityBps)})` : ''}`)
          .join(', ');
        lines.push(`候选 Top-3: ${cands}`);
      }
      if (rec.diagnostics) {
        const d = rec.diagnostics;
        lines.push(`诊断: round=${d.round} stage=${d.stage} duration=${formatMs(d.durationMs)} plan=${d.planType || '-'} expectedLabel=${d.expectedLabel || '(未收录)'}`);
        lines.push(`凭据版本: before=${d.credentialRevisionBefore ?? '-'} after=${d.credentialRevisionAfter ?? '-'} identityChanged=${Boolean(d.identityChanged)}`);
        if (d.detectorDecision || d.probabilityStatus) {
          lines.push(`评分器: decision=${d.detectorDecision || '-'} usedOutputs=${d.usedOutputs ?? '-'} probStatus=${d.probabilityStatus || '-'}`);
        }
        if (d.errorDetail) {
          lines.push(`错误详情: ${d.errorDetail}`);
        }
        for (const p of d.probes || []) {
          lines.push(
            `  Probe #${p.index} (${p.challengeId}, expect=${p.expectedCount}): ok=${p.ok} duration=${formatMs(p.durationMs)} ` +
            `batches=${p.batches} events=${p.events} parts=${p.textParts} textChars=${p.textChars} reasoningChars=${p.reasoningChars} ` +
            `finish=${p.finishReason || '-'} wire=${p.wireTerminal || '-'}` +
            (p.errorReason ? ` err=${p.errorReason}` : '') +
            (p.errorDetail ? ` detail=${p.errorDetail}` : '')
          );
        }
      } else {
        lines.push('诊断: (旧版本或排队记录，无逐探针持久化明细)');
      }
      lines.push('');
    }
    if ((bundle.runtimeLogs || []).length) {
      lines.push('=== 运行期事件日志（按时间顺序） ===');
      for (const entry of bundle.runtimeLogs) {
        const ts = new Date(entry.at).toISOString();
        const scope = [entry.category, entry.runId, entry.accountId].filter(Boolean).join(' ');
        const detail = entry.detail ? ` | ${JSON.stringify(entry.detail)}` : '';
        lines.push(`[${ts}] [${String(entry.level || 'info').toUpperCase()}] (${scope}) ${entry.message}${detail}`);
      }
    }
    return lines.join('\n');
  }

  function renderLogVisual(bundle) {
    const container = $('log-visual');
    const records = (bundle?.records || []).filter(r => r.diagnostics);
    if (!records.length || (!logTargetRunId && records.length > 3)) {
      container.hidden = true;
      container.replaceChildren();
      return;
    }
    const nodes = [];
    for (const rec of records) {
      const d = rec.diagnostics;
      const kv = make('div', null, 'diag-meta');
      const addKv = (label, val) => {
        const box = make('div', null, 'diag-kv');
        box.append(make('span', label), make('strong', val));
        kv.append(box);
      };
      addKv('任务 / 账号', `${rec.runId} · ${resolveRecordEmail(rec)}`);
      addKv('模型 / 档位', `${rec.model} / ${effortLabel(rec.effort)}${d.expectedLabel ? ` → ${d.expectedLabel}` : ' (仅排名)'}`);
      addKv('结束阶段 / 耗时', `${d.stage || '—'} · ${formatMs(d.durationMs)}`);
      addKv('凭据版本 / 身份', `${d.credentialRevisionBefore ?? '—'} → ${d.credentialRevisionAfter ?? '—'} (${d.identityChanged ? '身份变更' : '同账号身份'})`);
      if (d.detectorDecision || d.probabilityStatus) {
        addKv('评分器状态', `${d.detectorDecision || '—'} · 样本 ${d.usedOutputs ?? '—'}/3 · ${d.probabilityStatus || '—'}`);
      }
      nodes.push(kv);
      if (d.errorDetail) {
        nodes.push(make('div', `错误详情：${d.errorDetail}`, 'diag-err mono'));
      }
      if ((d.probes || []).length) {
        const tbl = make('table', null, 'probe-table');
        const thead = make('thead');
        const headRow = make('tr');
        for (const h of ['探针', '结果', '耗时', '批次 / 事件', '可见字符 / 推理字符', '结束原因 / 详情']) {
          headRow.append(make('th', h));
        }
        thead.append(headRow);
        const tbody = make('tbody');
        for (const p of d.probes) {
          const tr = make('tr');
          tr.append(
            make('td', `#${p.index} (${p.challengeId})`, 'mono'),
            make('td', p.ok ? '成功' : (p.errorReason || '失败')),
            make('td', formatMs(p.durationMs), 'mono'),
            make('td', `${p.batches} / ${p.events}`, 'mono'),
            make('td', `${p.textChars} (${p.textParts} 片) / ${p.reasoningChars}`, 'mono'),
            make('td', [p.finishReason, p.wireTerminal, p.errorDetail].filter(Boolean).join(' · ') || '—', 'mono'),
          );
          tbody.append(tr);
        }
        tbl.append(thead, tbody);
        nodes.push(tbl);
      }
    }
    container.replaceChildren(...nodes);
    container.hidden = false;
  }

  function renderLogOutput() {
    if (!logBundle) return;
    const fmt = $('log-format').value;
    const text = fmt === 'text' ? formatLogText(logBundle) : JSON.stringify(logBundle, null, 2);
    $('log-output').value = text;
    renderLogVisual(logBundle);
    const kb = (new TextEncoder().encode(text).byteLength / 1024).toFixed(1);
    $('log-download').hidden = !canDownloadFile;
    const sandboxHint = canDownloadFile
      ? ''
      : '（当前宿主沙箱禁止直接下载文件，请使用「一键复制日志」）';
    $('log-notice').textContent = `已采集 ${logBundle.recordCount} 条任务记录与 ${logBundle.runtimeLogCount} 条运行日志（${kb} KB）${sandboxHint}`;
  }

  async function loadLogs() {
    $('log-notice').textContent = '正在采集诊断日志…';
    $('log-output').value = '';
    try {
      const params = {};
      if (logTargetRunId) {
        params.runId = logTargetRunId;
      } else if ($('log-scope').value === 'all') {
        params.scope = 'all';
      } else {
        params.search = queryState.search;
        params.source = queryState.source;
        params.verdict = queryState.verdict;
        params.model = queryState.model;
        params.effort = queryState.effort;
      }
      logBundle = await request('GET', 'logs', null, params);
      renderLogOutput();
    } catch (error) {
      $('log-notice').textContent = `采集日志失败：${reasonText(error.message)}`;
    }
  }

  function openLogs(runId = null) {
    logTargetRunId = runId || null;
    $('log-scope').hidden = Boolean(logTargetRunId);
    $('log-drawer-title').textContent = logTargetRunId
      ? `单条任务诊断日志 · ${logTargetRunId}`
      : '诊断日志导出';
    $('log-drawer').hidden = false;
    $('log-drawer').scrollIntoView({ behavior: 'smooth', block: 'nearest' });
    loadLogs();
  }

  async function copyLogs() {
    const text = $('log-output').value;
    if (!text) return;
    const kb = (new TextEncoder().encode(text).byteLength / 1024).toFixed(1);
    let copied = false;
    if (navigator.clipboard && typeof navigator.clipboard.writeText === 'function') {
      try {
        await navigator.clipboard.writeText(text);
        copied = true;
      } catch {
        copied = false;
      }
    }
    if (!copied) {
      const area = $('log-output');
      area.focus();
      area.select();
      area.setSelectionRange(0, text.length);
      try {
        copied = document.execCommand('copy');
      } catch {
        copied = false;
      }
    }
    $('log-notice').textContent = copied
      ? `已复制完整诊断日志到剪贴板（${kb} KB），可直接粘贴发送排查。`
      : '未能自动写入剪贴板，已自动全选下方文本框，请按 Ctrl+C / Cmd+C 复制。';
  }

  function downloadLogs() {
    const text = $('log-output').value;
    if (!text) return;
    const isText = $('log-format').value === 'text';
    const ext = isText ? 'txt' : 'json';
    const stampStr = new Date().toISOString().replace(/[:.]/g, '-');
    const name = logTargetRunId
      ? `cpr-iq-test-log-${logTargetRunId}.${ext}`
      : `cpr-iq-test-logs-${stampStr}.${ext}`;
    const blob = new Blob([text], { type: isText ? 'text/plain;charset=utf-8' : 'application/json;charset=utf-8' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = name;
    document.body.append(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(url), 5000);
  }

  $('export-logs').addEventListener('click', () => openLogs(null));
  $('log-scope').addEventListener('change', () => loadLogs());
  $('log-format').addEventListener('change', () => renderLogOutput());
  $('log-refresh').addEventListener('click', () => loadLogs());
  $('log-close').addEventListener('click', () => {
    $('log-drawer').hidden = true;
    logTargetRunId = null;
  });
  $('log-copy').addEventListener('click', () => copyLogs());
  $('log-select').addEventListener('click', () => {
    const area = $('log-output');
    area.focus();
    area.select();
    area.setSelectionRange(0, area.value.length);
    $('log-notice').textContent = '已全选日志内容，可按 Ctrl+C / Cmd+C 复制。';
  });
  $('log-download').addEventListener('click', () => downloadLogs());

  refresh(true, false);
  const timer = setInterval(() => {
    if (!document.hidden && !busy) refresh(!schedule, false);
  }, 5000);
  window.addEventListener('pagehide', () => clearInterval(timer), { once: true });
})();
