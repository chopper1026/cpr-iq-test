'use strict';
(() => {
  const $ = id => document.getElementById(id);
  const bridge = window.codexProxyPlugin;
  const SVG_NS = 'http://www.w3.org/2000/svg';

  const reasons = {
    invalidInput: '请检查所选 Client Key、模型、推理档位、巡检时间与账号配置是否有效。',
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
    queued: '排队中',
    untested: '未检测',
  };

  const verdictTagClass = {
    match: 'z-tag-success',
    ranked: 'z-tag-info',
    running: 'z-tag-primary',
    queued: 'z-tag-info',
    suspect: 'z-tag-danger',
    insufficient: 'z-tag-warning',
    failed: 'z-tag-danger',
    interrupted: 'z-tag-danger',
    skipped: 'z-tag-default',
    untested: 'z-tag-default',
  };

  const statusPriority = {
    running: 0,
    queued: 1,
    suspect: 2,
    failed: 3,
    interrupted: 4,
    insufficient: 5,
    skipped: 6,
    untested: 7,
    ranked: 8,
    match: 9,
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

  const credentialStateLabels = {
    ready: '凭据就绪',
    refreshing: '凭据刷新中',
    expired: '凭据已过期',
    invalid: '凭据无效',
    revoked: '凭据已吊销',
  };

  const HOUR_OPTIONS = Array.from({ length: 24 }, (_, h) => {
    const str = String(h).padStart(2, '0');
    return { value: str, label: `${str} 时` };
  });
  const MINUTE_OPTIONS = Array.from({ length: 60 }, (_, m) => {
    const str = String(m).padStart(2, '0');
    return { value: str, label: `${str} 分` };
  });

  const browserOffsetMinutes = -new Date().getTimezoneOffset();

  function formatUtcOffset(minutes) {
    const sign = minutes >= 0 ? '+' : '-';
    const abs = Math.abs(minutes);
    const hh = String(Math.floor(abs / 60)).padStart(2, '0');
    const mm = String(abs % 60).padStart(2, '0');
    return `UTC${sign}${hh}:${mm}`;
  }

  function buildTimezoneOptions(extraOffset = null) {
    const standard = [
      [-720, 'UTC-12:00'],
      [-600, 'UTC-10:00（檀香山）'],
      [-480, 'UTC-08:00（洛杉矶）'],
      [-420, 'UTC-07:00（丹佛）'],
      [-360, 'UTC-06:00（芝加哥）'],
      [-300, 'UTC-05:00（纽约）'],
      [-180, 'UTC-03:00（圣保罗）'],
      [0, 'UTC+00:00（标准时间）'],
      [60, 'UTC+01:00（柏林 / 巴黎）'],
      [120, 'UTC+02:00（雅典）'],
      [180, 'UTC+03:00（莫斯科）'],
      [240, 'UTC+04:00（迪拜）'],
      [330, 'UTC+05:30（孟买）'],
      [420, 'UTC+07:00（曼谷）'],
      [480, 'UTC+08:00（北京 / 上海 / 新加坡）'],
      [540, 'UTC+09:00（东京 / 首尔）'],
      [600, 'UTC+10:00（悉尼）'],
      [720, 'UTC+12:00（奥克兰）'],
    ];
    const map = new Map(standard);
    if (!map.has(browserOffsetMinutes) && browserOffsetMinutes % 15 === 0) {
      map.set(browserOffsetMinutes, formatUtcOffset(browserOffsetMinutes));
    }
    if (extraOffset != null && !map.has(extraOffset) && extraOffset % 15 === 0) {
      map.set(extraOffset, formatUtcOffset(extraOffset));
    }
    return [...map.entries()]
      .sort((a, b) => a[0] - b[0])
      .map(([val, label]) => ({
        value: String(val),
        label,
        description: val === browserOffsetMinutes ? '浏览器本地时区' : '',
      }));
  }

  const stamp = new Intl.DateTimeFormat('zh-CN', { dateStyle: 'medium', timeStyle: 'short', hour12: false });
  const shortTime = new Intl.DateTimeFormat('zh-CN', { timeStyle: 'medium', hour12: false });
  const localDate = at => at == null ? '—' : stamp.format(new Date(at));
  const reasonText = code => reasons[code] || '请求失败，请刷新后重试。';
  const effortLabel = effort => effortLabels[effort] || effort || '默认';

  function relativeTime(at, nowMs = Date.now()) {
    if (at == null) return '—';
    const diffSec = Math.max(0, Math.floor((nowMs - at) / 1000));
    if (diffSec < 45) return '刚刚';
    if (diffSec < 3600) return `${Math.floor(diffSec / 60)} 分钟前`;
    if (diffSec < 86400) return `${Math.floor(diffSec / 3600)} 小时前`;
    const days = Math.floor(diffSec / 86400);
    return `${days} 天前`;
  }

  const make = (tag, text, className) => {
    const el = document.createElement(tag);
    if (text != null) el.textContent = text;
    if (className) el.className = className;
    return el;
  };

  function makeSvg(size, paths) {
    const svg = document.createElementNS(SVG_NS, 'svg');
    svg.setAttribute('width', String(size));
    svg.setAttribute('height', String(size));
    svg.setAttribute('viewBox', '0 0 24 24');
    svg.setAttribute('fill', 'none');
    svg.setAttribute('stroke', 'currentColor');
    svg.setAttribute('stroke-width', '2');
    svg.setAttribute('stroke-linecap', 'round');
    svg.setAttribute('stroke-linejoin', 'round');
    svg.setAttribute('aria-hidden', 'true');
    for (const d of paths) {
      const path = document.createElementNS(SVG_NS, 'path');
      path.setAttribute('d', d);
      svg.append(path);
    }
    return svg;
  }

  const chevronIcon = () => makeSvg(15, ['m6 9 6 6 6-6']);
  const checkIcon = (size = 14) => makeSvg(size, ['M20 6 9 17l-5-5']);
  const closeIcon = () => makeSvg(14, ['M18 6 6 18', 'm6 6 12 12']);

  /* ---------------- Codex Proxy ZSelect component ---------------- */
  let openSelect = null;
  let selectSeq = 0;

  function createZSelect(config = {}) {
    selectSeq += 1;
    const id = `z-sel-${selectSeq}`;
    const size = config.size || 'default';
    const filterable = Boolean(config.filterable);
    const placeholder = config.placeholder || '请选择';
    let options = config.options ? [...config.options] : [];
    let value = config.value != null ? String(config.value) : '';
    let disabled = Boolean(config.disabled);
    let isOpen = false;
    let search = '';
    let highlighted = -1;

    const root = make('div', null, `z-select${size === 'small' ? ' is-small' : ''}`);
    const trigger = make('button', null, 'z-select-trigger');
    trigger.type = 'button';
    trigger.setAttribute('role', 'combobox');
    trigger.setAttribute('aria-haspopup', 'listbox');
    trigger.setAttribute('aria-expanded', 'false');
    if (config.ariaLabel) trigger.setAttribute('aria-label', config.ariaLabel);
    trigger.disabled = disabled;

    const valueWrap = make('span', null, 'z-select-value');
    const labelSpan = make('span', '', 'z-select-label');
    const descSpan = make('span', '', 'z-select-desc');
    valueWrap.append(labelSpan, descSpan);
    const caret = make('span', null, 'z-select-caret');
    caret.append(chevronIcon());
    trigger.append(valueWrap, caret);
    root.append(trigger);

    const popover = make('div', null, `z-select-popover${size === 'small' ? ' is-small' : ''}`);
    popover.hidden = true;
    let searchInput = null;
    if (filterable) {
      const searchBox = make('div', null, 'z-select-search-box');
      searchInput = make('input', null, 'z-input z-input-sm');
      searchInput.type = 'search';
      searchInput.placeholder = config.searchPlaceholder || '搜索选项…';
      searchInput.autocomplete = 'off';
      searchInput.addEventListener('input', () => {
        search = searchInput.value;
        highlighted = 0;
        renderOptions();
        positionPopover();
      });
      searchInput.addEventListener('keydown', handleKeydown);
      searchBox.append(searchInput);
      popover.append(searchBox);
    }
    const listEl = make('div', null, 'z-select-list');
    listEl.id = `${id}-listbox`;
    listEl.setAttribute('role', 'listbox');
    popover.append(listEl);

    function visibleOptions() {
      const q = search.trim().toLowerCase();
      if (!q) return options;
      return options.filter(opt =>
        String(opt.label || '').toLowerCase().includes(q) ||
        String(opt.value || '').toLowerCase().includes(q) ||
        String(opt.description || '').toLowerCase().includes(q) ||
        String(opt.badge || '').toLowerCase().includes(q)
      );
    }

    function renderTrigger() {
      const selected = options.find(o => String(o.value) === value);
      if (selected) {
        labelSpan.textContent = selected.label;
        labelSpan.classList.remove('is-placeholder');
        if (selected.description) {
          descSpan.textContent = selected.description;
          descSpan.hidden = false;
        } else {
          descSpan.textContent = '';
          descSpan.hidden = true;
        }
        trigger.title = selected.description ? `${selected.label} — ${selected.description}` : selected.label;
      } else {
        labelSpan.textContent = placeholder;
        labelSpan.classList.add('is-placeholder');
        descSpan.textContent = '';
        descSpan.hidden = true;
        trigger.title = placeholder;
      }
      trigger.disabled = disabled;
    }

    function renderOptions() {
      const shown = visibleOptions();
      if (!shown.length) {
        listEl.replaceChildren(make('div', search ? '无匹配选项' : '暂无可选项', 'z-select-empty'));
        return;
      }
      if (highlighted >= shown.length) highlighted = shown.length - 1;
      listEl.replaceChildren(...shown.map((opt, idx) => {
        const selected = String(opt.value) === value;
        const btn = make('button', null, `z-select-option${selected ? ' is-selected' : ''}${idx === highlighted ? ' is-highlighted' : ''}`);
        btn.type = 'button';
        btn.setAttribute('role', 'option');
        btn.setAttribute('aria-selected', String(selected));
        btn.disabled = Boolean(opt.disabled);

        btn.append(make('span', opt.label, 'z-select-opt-main'));
        if (opt.description) {
          btn.append(make('span', opt.description, 'z-select-opt-desc'));
        }
        if (opt.badge) {
          btn.append(make('span', opt.badge, 'z-select-opt-badge'));
        }
        if (selected) {
          const mark = make('span', null, 'z-select-opt-check');
          mark.append(checkIcon(14));
          btn.append(mark);
        }
        btn.addEventListener('mouseenter', () => {
          if (!opt.disabled) highlighted = idx;
        });
        btn.addEventListener('mousedown', e => e.preventDefault());
        btn.addEventListener('click', () => {
          if (opt.disabled) return;
          selectValue(String(opt.value));
        });
        return btn;
      }));
    }

    function positionPopover() {
      const rect = trigger.getBoundingClientRect();
      const viewHeight = window.innerHeight || document.documentElement.clientHeight;
      const viewWidth = window.innerWidth || document.documentElement.clientWidth;
      const minW = Math.max(rect.width, config.minMenuWidth || 0);
      const width = Math.min(minW, viewWidth - 16);
      const left = Math.max(8, Math.min(rect.left, viewWidth - width - 8));
      const spaceBelow = viewHeight - rect.bottom - 12;
      const spaceAbove = rect.top - 12;
      const openTop = spaceBelow < 180 && spaceAbove > spaceBelow;

      popover.style.width = `${Math.round(width)}px`;
      popover.style.left = `${Math.round(left)}px`;
      popover.style.bottom = 'auto';
      popover.classList.toggle('is-top', openTop);
      const maxH = Math.max(110, Math.min(272, (openTop ? spaceAbove : spaceBelow) - (filterable ? 50 : 10)));
      listEl.style.maxHeight = `${Math.round(maxH)}px`;

      if (openTop) {
        const menuHeight = popover.getBoundingClientRect().height || (maxH + (filterable ? 48 : 8));
        popover.style.top = `${Math.max(8, Math.round(rect.top - menuHeight - 6))}px`;
      } else {
        popover.style.top = `${Math.round(rect.bottom + 6)}px`;
      }
    }

    function open() {
      if (disabled || isOpen) return;
      if (openSelect && openSelect !== api) openSelect.close();
      isOpen = true;
      openSelect = api;
      search = '';
      if (searchInput) searchInput.value = '';
      const idx = options.findIndex(o => String(o.value) === value);
      highlighted = idx >= 0 ? idx : 0;
      root.classList.add('is-open');
      trigger.setAttribute('aria-expanded', 'true');
      trigger.setAttribute('aria-controls', listEl.id);
      if (!popover.isConnected) document.body.append(popover);
      popover.hidden = false;
      renderOptions();
      positionPopover();
      requestAnimationFrame(() => {
        if (isOpen) popover.classList.add('is-visible');
      });
      if (searchInput) {
        searchInput.focus({ preventScroll: true });
      }
      const activeBtn = listEl.querySelector('.is-selected');
      if (activeBtn) activeBtn.scrollIntoView({ block: 'nearest' });
    }

    function close(restoreFocus = false) {
      if (!isOpen) return;
      isOpen = false;
      if (openSelect === api) openSelect = null;
      root.classList.remove('is-open');
      trigger.setAttribute('aria-expanded', 'false');
      trigger.removeAttribute('aria-controls');
      popover.classList.remove('is-visible');
      popover.hidden = true;
      if (restoreFocus) trigger.focus({ preventScroll: true });
    }

    function selectValue(nextVal) {
      const changed = value !== nextVal;
      value = nextVal;
      renderTrigger();
      close(true);
      if (changed && typeof config.onChange === 'function') {
        config.onChange(value);
      }
    }

    function moveHighlight(delta) {
      const shown = visibleOptions();
      if (!shown.length) return;
      let next = highlighted;
      for (let step = 0; step < shown.length; step += 1) {
        next = (next + delta + shown.length) % shown.length;
        if (!shown[next].disabled) {
          highlighted = next;
          renderOptions();
          const btn = listEl.children[highlighted];
          if (btn) btn.scrollIntoView({ block: 'nearest' });
          return;
        }
      }
    }

    function handleKeydown(e) {
      if (e.key === 'Escape') {
        e.preventDefault();
        close(true);
        return;
      }
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        if (!isOpen) open();
        else moveHighlight(1);
        return;
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        if (!isOpen) open();
        else moveHighlight(-1);
        return;
      }
      if (e.key === 'Enter') {
        if (isOpen) {
          e.preventDefault();
          const shown = visibleOptions();
          const target = shown[highlighted];
          if (target && !target.disabled) selectValue(String(target.value));
        } else {
          e.preventDefault();
          open();
        }
        return;
      }
      if (e.key === ' ' && e.target === trigger) {
        e.preventDefault();
        if (isOpen) close();
        else open();
      }
    }

    trigger.addEventListener('click', () => {
      if (isOpen) close();
      else open();
    });
    trigger.addEventListener('keydown', handleKeydown);

    const api = {
      el: root,
      popover,
      getValue: () => value,
      setValue(nextVal, silent = true) {
        const str = nextVal == null ? '' : String(nextVal);
        const changed = value !== str;
        value = str;
        renderTrigger();
        if (isOpen) renderOptions();
        if (changed && !silent && typeof config.onChange === 'function') {
          config.onChange(value);
        }
      },
      setOptions(nextOpts, keepValue = true) {
        options = [...nextOpts];
        if (!keepValue && !options.some(o => String(o.value) === value)) {
          value = options.length ? String(options[0].value) : '';
        }
        renderTrigger();
        if (isOpen) {
          renderOptions();
          positionPopover();
        }
      },
      setDisabled(flag) {
        disabled = Boolean(flag);
        if (disabled) close();
        renderTrigger();
      },
      open,
      close,
      contains(node) {
        return root.contains(node) || popover.contains(node);
      },
      destroy() {
        close();
        popover.remove();
        root.remove();
      },
    };

    renderTrigger();
    return api;
  }

  document.addEventListener('pointerdown', e => {
    if (openSelect && !openSelect.contains(e.target)) {
      openSelect.close();
    }
  });
  window.addEventListener('resize', () => {
    if (openSelect) openSelect.close();
  });

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
    if (!document.documentElement.dataset.theme) {
      document.documentElement.dataset.theme = bridge.theme === 'dark' ? 'dark' : 'light';
    }
  }
  window.addEventListener('codex-proxy-themechange', () => {
    document.documentElement.dataset.theme = bridge.theme === 'dark' ? 'dark' : 'light';
  });
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

  /* ---------------- Application State ---------------- */
  let catalog = { accounts: [], keys: [], models: [], maxBatch: 100, detectorReady: false };
  let accountStatuses = null;
  let statusByAccountId = new Map();
  let schedule = null;
  let runsData = null;
  let activeTab = 'status';
  let busy = false;
  let polling = false;
  let nonce = null;
  let searchTimer = null;
  let statusSearchTimer = null;
  let logBundle = null;
  let logTargetRunId = null;
  let scheduleMode = 'interval';
  let dailySlotRows = [];
  let scheduleBaseline = null;
  let scheduleFeedback = null;
  let scheduleEpoch = 0;
  const canDownloadFile = window.origin !== 'null';

  const statusFilter = {
    verdict: '',
    search: '',
    plan: '',
    sort: 'priority',
  };

  const queryState = {
    page: 1,
    pageSize: 20,
    search: '',
    source: '',
    verdict: '',
    model: '',
    effort: '',
    accountId: '',
    accountLabel: '',
  };

  /* ---------------- Tabs & KPI Navigation ---------------- */
  function setTab(name) {
    if (openSelect) openSelect.close();
    activeTab = name;
    for (const key of ['status', 'detect', 'records']) {
      const active = name === key;
      $(`tab-${key}`).classList.toggle('is-active', active);
      $(`tab-${key}`).setAttribute('aria-selected', String(active));
      $(`panel-${key}`).hidden = !active;
    }
    if (name === 'records') fetchRuns();
    if (name === 'status') fetchAccountStatuses();
  }

  $('tab-status').addEventListener('click', () => setTab('status'));
  $('tab-detect').addEventListener('click', () => setTab('detect'));
  $('tab-records').addEventListener('click', () => setTab('records'));
  $('goto-records').addEventListener('click', () => setTab('records'));
  $('goto-detect-from-status').addEventListener('click', () => setTab('detect'));
  $('kpi-status-card').addEventListener('click', () => setTab('status'));
  $('kpi-queue-card').addEventListener('click', () => setTab('records'));
  $('kpi-schedule-card').addEventListener('click', () => setTab('detect'));
  $('summary-records-card').addEventListener('click', () => setTab('records'));

  function formatAccountPrimary(account) {
    if (account.email) return account.email;
    if (account.name && account.name !== account.id) return account.name;
    return account.id || account.accountId || '—';
  }

  function makeVerdictTag(code, labelOverride = null, extraClass = '') {
    const cls = verdictTagClass[code] || 'z-tag-default';
    const tag = make('span', null, `z-tag ${cls}${extraClass ? ` ${extraClass}` : ''}`);
    if (code === 'running') {
      tag.append(make('span', null, 'tag-dot is-pulsing'));
    }
    tag.append(document.createTextNode(labelOverride || verdictLabels[code] || code));
    return tag;
  }

  function formatProb(bps) {
    if (bps == null) return '';
    return `${(bps / 100).toFixed(1)}%`;
  }

  function formatDuration(ms) {
    if (ms == null) return '';
    return ms >= 1000 ? `${(ms / 1000).toFixed(1)}s` : `${ms}ms`;
  }

  /* ---------------- Reusable Target Controls (Manual & Schedule) ---------------- */
  function createTargetControls(containerId, onChange = null) {
    const selectedIds = new Set();
    const notifyChange = () => {
      if (typeof onChange === 'function') onChange();
      updateButtons();
    };

    const keySelect = createZSelect({
      placeholder: '选择已启用 Client Key',
      ariaLabel: '测试 Client Key',
      onChange: notifyChange,
    });
    const modelSelect = createZSelect({
      placeholder: '选择模型',
      ariaLabel: '检测模型',
      filterable: true,
      searchPlaceholder: '搜索模型名称或 ID…',
      minMenuWidth: 260,
      onChange: () => {
        syncEfforts(ctrl);
        notifyChange();
      },
    });
    const effortSelect = createZSelect({
      placeholder: '请先选择模型',
      ariaLabel: '推理档位',
      onChange: notifyChange,
    });

    const keyGroup = make('div', null, 'field-group');
    keyGroup.append(make('span', '测试 Client Key', 'field-label'), keySelect.el);

    const modelGroup = make('div', null, 'field-group');
    modelGroup.append(make('span', '检测模型', 'field-label'), modelSelect.el);

    const effortGroup = make('div', null, 'field-group');
    effortGroup.append(make('span', '推理档位', 'field-label'), effortSelect.el);

    const modelRow = make('div', null, 'fields-row');
    modelRow.append(modelGroup, effortGroup);

    const searchWrap = make('div', null, 'z-input-wrap account-search-wrap');
    const searchIcon = make('span', null, 'z-input-icon');
    searchIcon.append(makeSvg(14, ['M11 3a8 8 0 1 0 0 16 8 8 0 1 0 0-16', 'm21 21-4.3-4.3']));
    const search = make('input', null, 'z-input z-input-sm has-icon');
    search.type = 'search';
    search.placeholder = '搜索账号邮箱、名称、套餐或 ID…';
    searchWrap.append(searchIcon, search);

    const accounts = make('div', null, 'accounts');
    const selectAllBtn = make('button', '全选', 'z-btn z-btn-plain z-btn-xs');
    selectAllBtn.type = 'button';
    const selectAttentionBtn = make('button', '选异常/未测', 'z-btn z-btn-plain z-btn-xs');
    selectAttentionBtn.type = 'button';
    const clearBtn = make('button', '清空', 'z-btn z-btn-plain z-btn-xs');
    clearBtn.type = 'button';

    const legend = make('legend', '指定账号');
    const tools = make('div', null, 'account-tools');
    tools.append(selectAllBtn, selectAttentionBtn, clearBtn);
    const head = make('div', null, 'account-head');
    head.append(legend, tools);

    const group = make('fieldset');
    group.append(head, searchWrap, accounts);
    $(containerId).append(keyGroup, modelRow, group);

    const ctrl = {
      key: keySelect,
      model: modelSelect,
      effort: effortSelect,
      search,
      accounts,
      legend,
      selectAllBtn,
      selectAttentionBtn,
      clearBtn,
      selectedIds,
      notifyChange,
    };

    search.addEventListener('input', () => renderAccountList(ctrl));
    selectAllBtn.addEventListener('click', () => {
      const visible = filteredAccounts(search.value);
      for (const a of visible) {
        if (selectedIds.size >= catalog.maxBatch) break;
        selectedIds.add(a.id);
      }
      renderAccountList(ctrl);
      notifyChange();
    });
    selectAttentionBtn.addEventListener('click', () => {
      const visible = filteredAccounts(search.value);
      selectedIds.clear();
      for (const a of visible) {
        if (selectedIds.size >= catalog.maxBatch) break;
        const st = statusByAccountId.get(a.id)?.liveStatus || 'untested';
        if (['untested', 'suspect', 'failed', 'interrupted', 'insufficient'].includes(st)) {
          selectedIds.add(a.id);
        }
      }
      renderAccountList(ctrl);
      notifyChange();
    });
    clearBtn.addEventListener('click', () => {
      selectedIds.clear();
      renderAccountList(ctrl);
      notifyChange();
    });

    return ctrl;
  }

  function clearScheduleFeedback() {
    scheduleFeedback = null;
  }

  const manual = createTargetControls('manual-target');
  const planned = createTargetControls('schedule-target', clearScheduleFeedback);

  /* ---------------- Schedule Controls (Interval + Daily Fixed Time) ---------------- */
  const scheduleHoursSelect = createZSelect({
    ariaLabel: '巡检间隔',
    value: '24',
    options: [
      { value: '1', label: '每 1 小时' },
      { value: '6', label: '每 6 小时' },
      { value: '12', label: '每 12 小时' },
      { value: '24', label: '每 24 小时' },
      { value: '48', label: '每 48 小时' },
      { value: '168', label: '每 7 天' },
    ],
    onChange: () => {
      clearScheduleFeedback();
      updateButtons();
    },
  });
  $('schedule-hours-host').append(scheduleHoursSelect.el);

  const scheduleTzSelect = createZSelect({
    ariaLabel: '执行时区',
    filterable: true,
    searchPlaceholder: '搜索时区或城市…',
    value: String(browserOffsetMinutes),
    options: buildTimezoneOptions(browserOffsetMinutes),
    onChange: () => {
      clearScheduleFeedback();
      updateButtons();
    },
  });
  $('schedule-tz-host').append(scheduleTzSelect.el);

  function setScheduleMode(mode, silent = false) {
    scheduleMode = mode === 'daily' ? 'daily' : 'interval';
    for (const btn of $('schedule-mode-group').querySelectorAll('.z-seg-item')) {
      const active = btn.dataset.mode === scheduleMode;
      btn.classList.toggle('is-active', active);
      btn.setAttribute('aria-checked', String(active));
    }
    $('schedule-interval-panel').hidden = scheduleMode !== 'interval';
    $('schedule-daily-panel').hidden = scheduleMode !== 'daily';
    if (!silent) updateButtons();
  }

  $('schedule-mode-group').addEventListener('click', e => {
    const btn = e.target.closest('.z-seg-item');
    if (!btn) return;
    clearScheduleFeedback();
    setScheduleMode(btn.dataset.mode);
  });

  function clearDailySlotRows() {
    for (const row of dailySlotRows) {
      row.hourSelect.destroy();
      row.minuteSelect.destroy();
    }
    dailySlotRows = [];
    $('daily-slots').replaceChildren();
  }

  function renderDailySlots(times) {
    const list = (times && times.length ? times : ['09:00']).slice(0, 12);
    clearDailySlotRows();
    for (const timeStr of list) {
      const [hh = '09', mm = '00'] = String(timeStr).split(':');
      const rowEl = make('div', null, 'daily-slot-row');

      const hourWrap = make('div', null, 'time-part');
      const hourSelect = createZSelect({
        size: 'small',
        ariaLabel: '小时',
        value: hh.padStart(2, '0'),
        options: HOUR_OPTIONS,
        onChange: () => {
          clearScheduleFeedback();
          updateButtons();
        },
      });
      hourWrap.append(hourSelect.el);

      const minWrap = make('div', null, 'time-part');
      const minuteSelect = createZSelect({
        size: 'small',
        ariaLabel: '分钟',
        value: mm.padStart(2, '0'),
        options: MINUTE_OPTIONS,
        onChange: () => {
          clearScheduleFeedback();
          updateButtons();
        },
      });
      minWrap.append(minuteSelect.el);

      rowEl.append(hourWrap, make('span', ':', 'daily-colon'), minWrap);

      if (list.length > 1) {
        const rmBtn = make('button', null, 'z-btn z-btn-plain z-btn-xs');
        rmBtn.type = 'button';
        rmBtn.title = '移除该时间点';
        rmBtn.setAttribute('aria-label', '移除该时间点');
        rmBtn.append(closeIcon());
        rmBtn.addEventListener('click', () => {
          const remaining = readDailyTimes().filter((_, i) => i !== dailySlotRows.indexOf(entry));
          renderDailySlots(remaining.length ? remaining : ['09:00']);
          clearScheduleFeedback();
          updateButtons();
        });
        rowEl.append(rmBtn);
      }

      const entry = { rowEl, hourSelect, minuteSelect };
      dailySlotRows.push(entry);
      $('daily-slots').append(rowEl);
    }
    $('add-daily-time').disabled = dailySlotRows.length >= 12;
  }

  function readDailyTimes() {
    return dailySlotRows.map(row => `${row.hourSelect.getValue() || '09'}:${row.minuteSelect.getValue() || '00'}`);
  }

  function hasDuplicateDailyTimes(times) {
    return new Set(times).size !== times.length;
  }

  $('add-daily-time').addEventListener('click', () => {
    const current = readDailyTimes();
    if (current.length >= 12) return;
    const used = new Set(current);
    const preferred = ['21:00', '03:00', '15:00', '12:00', '18:00', '06:00', '00:00'];
    let candidate = preferred.find(t => !used.has(t));
    if (!candidate) {
      for (let h = 0; h < 24; h += 1) {
        const t = `${String(h).padStart(2, '0')}:00`;
        if (!used.has(t)) {
          candidate = t;
          break;
        }
      }
    }
    renderDailySlots([...current, candidate || '09:30']);
    clearScheduleFeedback();
    updateButtons();
  });

  renderDailySlots(['09:00']);

  /* ---------------- Status Tab Filter Dropdowns ---------------- */
  const statusPlanSelect = createZSelect({
    size: 'small',
    ariaLabel: '按套餐筛选',
    value: '',
    options: [{ value: '', label: '全部套餐' }],
    onChange: val => {
      statusFilter.plan = val;
      renderAccountStatusView();
    },
  });
  $('status-plan-host').append(statusPlanSelect.el);

  const statusSortSelect = createZSelect({
    size: 'small',
    ariaLabel: '账号排序方式',
    value: 'priority',
    options: [
      { value: 'priority', label: '按状态优先排序' },
      { value: 'recent', label: '按最近检测时间' },
      { value: 'name', label: '按账号邮箱排序' },
    ],
    onChange: val => {
      statusFilter.sort = val || 'priority';
      renderAccountStatusView();
    },
  });
  $('status-sort-host').append(statusSortSelect.el);

  /* ---------------- Records Tab Filter & Drawer Dropdowns ---------------- */
  const filterSourceSelect = createZSelect({
    size: 'small',
    ariaLabel: '按触发来源筛选',
    value: '',
    options: [
      { value: '', label: '全部来源' },
      { value: 'manual', label: '手动检测' },
      { value: 'scheduled', label: '定时巡检' },
    ],
    onChange: val => {
      queryState.source = val;
      queryState.page = 1;
      fetchRuns();
    },
  });
  $('filter-source-host').append(filterSourceSelect.el);

  const filterVerdictSelect = createZSelect({
    size: 'small',
    ariaLabel: '按检测状态筛选',
    value: '',
    options: [
      { value: '', label: '全部状态' },
      { value: 'running', label: '检测中' },
      { value: 'queued', label: '排队中' },
      { value: 'match', label: '指纹匹配' },
      { value: 'ranked', label: '参考排名' },
      { value: 'suspect', label: '疑似指纹异常' },
      { value: 'insufficient', label: '证据不足' },
      { value: 'failed', label: '检测失败' },
      { value: 'interrupted', label: '已中断' },
      { value: 'skipped', label: '已跳过' },
    ],
    onChange: val => {
      queryState.verdict = val;
      queryState.page = 1;
      fetchRuns();
    },
  });
  $('filter-verdict-host').append(filterVerdictSelect.el);

  const filterModelSelect = createZSelect({
    size: 'small',
    ariaLabel: '按模型筛选',
    filterable: true,
    searchPlaceholder: '搜索模型…',
    minMenuWidth: 210,
    value: '',
    options: [{ value: '', label: '全部模型' }],
    onChange: val => {
      queryState.model = val;
      queryState.page = 1;
      fetchRuns();
    },
  });
  $('filter-model-host').append(filterModelSelect.el);

  const filterEffortSelect = createZSelect({
    size: 'small',
    ariaLabel: '按推理档位筛选',
    value: '',
    options: [{ value: '', label: '全部档位' }],
    onChange: val => {
      queryState.effort = val;
      queryState.page = 1;
      fetchRuns();
    },
  });
  $('filter-effort-host').append(filterEffortSelect.el);

  const pageSizeSelect = createZSelect({
    size: 'small',
    ariaLabel: '每页显示条数',
    value: '20',
    options: [
      { value: '10', label: '10 条' },
      { value: '20', label: '20 条' },
      { value: '50', label: '50 条' },
    ],
    onChange: val => {
      queryState.pageSize = Number(val) || 20;
      queryState.page = 1;
      fetchRuns();
    },
  });
  $('page-size-host').append(pageSizeSelect.el);

  const logScopeSelect = createZSelect({
    size: 'small',
    ariaLabel: '导出范围',
    value: 'filtered',
    options: [
      { value: 'filtered', label: '当前筛选记录' },
      { value: 'all', label: '全部任务与运行日志' },
    ],
    onChange: () => loadLogs(),
  });
  $('log-scope-host').append(logScopeSelect.el);

  const logFormatSelect = createZSelect({
    size: 'small',
    ariaLabel: '导出格式',
    value: 'json',
    options: [
      { value: 'json', label: 'JSON 完整日志' },
      { value: 'text', label: '纯文本诊断摘要' },
    ],
    onChange: () => renderLogOutput(),
  });
  $('log-format-host').append(logFormatSelect.el);

  /* ---------------- Account List Rendering (with Live Status Tags) ---------------- */
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
          ctrl.notifyChange();
        });

        const box = make('span', null, 'z-checkbox-box');
        box.setAttribute('aria-hidden', 'true');
        box.append(checkIcon(11));

        const primary = make('div', null, 'account-primary');
        primary.append(make('span', formatAccountPrimary(a)));
        if (a.name && a.name !== a.id && a.name !== a.email) {
          primary.append(make('span', a.name, 'z-tag z-tag-xs'));
        }
        if (a.planType) {
          primary.append(make('span', a.planType.toUpperCase(), 'z-tag z-tag-xs z-tag-primary'));
        }

        const topLine = make('div', null, 'account-top-line');
        topLine.append(primary);
        const live = statusByAccountId.get(a.id);
        if (live) {
          const liveCode = live.liveStatus;
          const timeSuffix = live.lastCheck && liveCode !== 'running' && liveCode !== 'queued'
            ? ` · ${relativeTime(live.lastCheck.endedAt)}`
            : '';
          topLine.append(makeVerdictTag(liveCode, `${verdictLabels[liveCode] || liveCode}${timeSuffix}`, 'z-tag-xs'));
        }

        const info = make('div', null, 'account-info');
        info.append(topLine);
        if (a.email || (a.name && a.name !== a.id)) {
          info.append(make('div', a.id, 'account-sub mono'));
        }

        const row = make('label', null, 'account');
        row.title = a.id;
        row.append(check, box, info);
        return row;
      }));
    }
    const total = catalog.accounts.length;
    const count = ctrl.selectedIds.size;
    ctrl.legend.textContent = `指定账号（已选 ${count} / 共 ${total}）`;
    ctrl.selectAllBtn.disabled = !visible.length || count >= catalog.maxBatch;
    ctrl.selectAttentionBtn.disabled = !visible.length;
    ctrl.clearBtn.disabled = count === 0;
  }

  function syncEfforts(ctrl, preferredEffort) {
    const currentEffort = preferredEffort != null ? preferredEffort : ctrl.effort.getValue();
    const selectedModel = catalog.models.find(m => m.id === ctrl.model.getValue());
    const efforts = selectedModel?.efforts?.length
      ? selectedModel.efforts
      : (currentEffort ? [currentEffort] : []);
    if (!efforts.length) {
      ctrl.effort.setOptions([{ value: '', label: '请先选择模型' }], false);
      ctrl.effort.setValue('');
      return;
    }
    ctrl.effort.setOptions(efforts.map(e => ({ value: e, label: effortLabel(e) })), true);
    if (efforts.includes(currentEffort)) {
      ctrl.effort.setValue(currentEffort);
    } else if (selectedModel && efforts.includes(selectedModel.defaultEffort)) {
      ctrl.effort.setValue(selectedModel.defaultEffort);
    } else {
      ctrl.effort.setValue(efforts[0]);
    }
  }

  function readTarget(ctrl) {
    return {
      clientKeyId: ctrl.key.getValue(),
      model: ctrl.model.getValue(),
      effort: ctrl.effort.getValue(),
      accountIds: [...ctrl.selectedIds].sort(),
    };
  }

  function isTargetUsable(t) {
    return Boolean(t.clientKeyId && t.model && t.effort && t.accountIds.length > 0 && t.accountIds.length <= catalog.maxBatch);
  }

  function applyTarget(ctrl, chosen) {
    const keyOpts = catalog.keys.length
      ? catalog.keys.map(k => ({
        value: k.id,
        label: k.name || k.id,
        description: k.name && k.name !== k.id ? k.id : '',
      }))
      : [{ value: '', label: '暂无已启用 Client Key', disabled: true }];
    ctrl.key.setOptions(keyOpts, true);
    const desiredKey = chosen.clientKeyId || (catalog.keys.length === 1 ? catalog.keys[0].id : '');
    ctrl.key.setValue(catalog.keys.some(k => k.id === desiredKey) ? desiredKey : '');

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
    const modelOpts = modelList.length
      ? modelList.map(m => ({
        value: m.id,
        label: m.id,
        description: m.displayName && m.displayName !== m.id ? m.displayName : '',
        badge: m.inReferenceBank ? '参考库' : '',
      }))
      : [{ value: '', label: '暂未获取到模型目录', disabled: true }];
    ctrl.model.setOptions(modelOpts, true);
    const desiredModel = chosen.model || (modelList.length ? modelList[0].id : '');
    ctrl.model.setValue(modelList.some(m => m.id === desiredModel) ? desiredModel : '');
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

  function describeScheduleShort(settings) {
    if (!settings || !settings.enabled) return '已关闭';
    if (settings.mode === 'daily') {
      const times = (settings.dailyTimes || ['09:00']).join('、');
      return `每日 ${times} (${formatUtcOffset(settings.utcOffsetMinutes || 0)})`;
    }
    const h = settings.hours || 24;
    return h === 168 ? '每 7 天' : `每 ${h} 小时`;
  }

  function readScheduleFormSnapshot() {
    const t = readTarget(planned);
    return {
      enabled: $('enabled').checked,
      mode: scheduleMode,
      hours: Number(scheduleHoursSelect.getValue()) || 24,
      dailyTimes: [...readDailyTimes()].sort(),
      utcOffsetMinutes: Number(scheduleTzSelect.getValue()) || 0,
      target: {
        clientKeyId: t.clientKeyId,
        model: t.model,
        effort: t.effort,
        accountIds: [...t.accountIds],
      },
    };
  }

  function isScheduleDirty() {
    if (!schedule || !scheduleBaseline) return false;
    const cur = readScheduleFormSnapshot();
    if (cur.enabled !== scheduleBaseline.enabled) return true;
    if (cur.mode !== scheduleBaseline.mode) return true;
    if (cur.mode === 'interval' && cur.hours !== scheduleBaseline.hours) return true;
    if (cur.mode === 'daily') {
      if (cur.utcOffsetMinutes !== scheduleBaseline.utcOffsetMinutes) return true;
      if (cur.dailyTimes.join(',') !== scheduleBaseline.dailyTimes.join(',')) return true;
    }
    const bt = scheduleBaseline.target;
    return (
      cur.target.clientKeyId !== bt.clientKeyId ||
      cur.target.model !== bt.model ||
      cur.target.effort !== bt.effort ||
      cur.target.accountIds.join(',') !== bt.accountIds.join(',')
    );
  }

  function validateScheduleForm(targetEnabled = $('enabled').checked) {
    const s = readTarget(planned);
    const dailyTimes = readDailyTimes();
    if (scheduleMode === 'daily' && hasDuplicateDailyTimes(dailyTimes)) {
      return {
        code: 'dupTimes',
        message: '存在重复的每日执行时间点，请修改后再保存。',
        focusEl: $('daily-slots'),
      };
    }
    if (!targetEnabled) return null;
    if (!catalog.detectorReady) {
      return {
        code: 'detectorUnavailable',
        message: '本地参考指纹库未就绪，暂时无法开启定时巡检。',
        focusEl: $('accessible-status'),
      };
    }
    if (!s.clientKeyId) {
      return {
        code: 'missingKey',
        message: catalog.keys.length
          ? '请先在下方选择「测试 Client Key」，再保存并开启定时巡检。'
          : '当前没有已启用的 Client Key，请先在宿主中创建并启用 Client Key。',
        focusEl: planned.key.el,
      };
    }
    if (!s.model || !s.effort) {
      return {
        code: 'missingModel',
        message: catalog.models.length
          ? '请先在下方选择「检测模型」与「推理档位」，再保存并开启定时巡检。'
          : '暂未获取到可用模型目录，请点击右上角「刷新目录」后重试。',
        focusEl: planned.model.el,
      };
    }
    if (s.accountIds.length === 0) {
      return {
        code: 'missingAccounts',
        message: catalog.accounts.length
          ? '请先在下方勾选至少 1 个「指定账号」，再点击「保存并开启」生效。'
          : '当前暂无启用的 OpenAI OAuth 账号，暂时无法开启定时巡检。',
        focusEl: planned.accounts.closest('fieldset'),
      };
    }
    if (s.accountIds.length > catalog.maxBatch) {
      return {
        code: 'tooManyAccounts',
        message: `单次巡检最多选择 ${catalog.maxBatch} 个账号，请减少所选账号后再保存。`,
        focusEl: planned.accounts.closest('fieldset'),
      };
    }
    return null;
  }

  function highlightScheduleTarget(el) {
    if (!el) return;
    el.scrollIntoView({ behavior: 'smooth', block: 'center' });
    el.classList.remove('is-attention-highlight');
    void el.offsetWidth;
    el.classList.add('is-attention-highlight');
    setTimeout(() => el.classList.remove('is-attention-highlight'), 1800);
  }

  function renderScheduleState() {
    const badge = $('schedule-badge');
    const banner = $('schedule-state-banner');
    const textEl = $('schedule-state-text');
    const locateBtn = $('schedule-banner-locate');
    const actionBtn = $('schedule-banner-action');

    if (!schedule) {
      badge.textContent = '加载中';
      badge.className = 'z-tag z-tag-xs';
      banner.hidden = true;
      return;
    }

    const persistedEnabled = Boolean(schedule.settings?.enabled);
    const draftEnabled = $('enabled').checked;
    const dirty = isScheduleDirty();
    const issue = validateScheduleForm(draftEnabled);

    if (persistedEnabled && !dirty) {
      badge.textContent = '已开启';
      badge.className = 'z-tag z-tag-xs z-tag-success';
    } else if (persistedEnabled && dirty) {
      badge.textContent = '已开启 · 有未保存修改';
      badge.className = 'z-tag z-tag-xs z-tag-warning';
    } else if (!persistedEnabled && draftEnabled) {
      badge.textContent = '未生效 · 待保存';
      badge.className = 'z-tag z-tag-xs z-tag-warning';
    } else if (!persistedEnabled && dirty) {
      badge.textContent = '已关闭 · 有未保存修改';
      badge.className = 'z-tag z-tag-xs z-tag-warning';
    } else {
      badge.textContent = '已关闭';
      badge.className = 'z-tag z-tag-xs';
    }

    locateBtn.hidden = true;
    actionBtn.hidden = true;
    actionBtn.disabled = busy;
    delete actionBtn.dataset.action;

    if (scheduleFeedback?.kind === 'error') {
      banner.hidden = false;
      banner.className = 'schedule-banner is-error';
      textEl.textContent = scheduleFeedback.text;
      if (issue?.focusEl) {
        locateBtn.hidden = false;
        locateBtn.textContent = issue.code === 'missingAccounts' ? '去选账号 ↓' : '去修改 ↓';
      }
      return;
    }

    if (draftEnabled && !persistedEnabled) {
      banner.hidden = false;
      banner.className = 'schedule-banner';
      if (issue) {
        textEl.textContent = `定时巡检尚未生效：${issue.message}`;
        if (issue.focusEl) {
          locateBtn.hidden = false;
          locateBtn.textContent = issue.code === 'missingAccounts' ? '去选账号 ↓' : '去补全设置 ↓';
        }
        if (
          issue.code === 'missingAccounts' &&
          catalog.accounts.length > 0 &&
          catalog.accounts.length <= catalog.maxBatch
        ) {
          actionBtn.hidden = false;
          actionBtn.textContent = `全选 ${catalog.accounts.length} 个账号并开启`;
          actionBtn.dataset.action = 'select-all-and-enable';
        }
      } else {
        textEl.textContent = '定时巡检尚未生效：配置已就绪，点击「保存并开启」立即生效。';
        actionBtn.hidden = false;
        actionBtn.textContent = '保存并开启';
        actionBtn.dataset.action = 'save';
      }
      return;
    }

    if (dirty) {
      banner.hidden = false;
      if (issue) {
        banner.className = 'schedule-banner';
        textEl.textContent = `有未保存修改：${issue.message}`;
        if (issue.focusEl) {
          locateBtn.hidden = false;
          locateBtn.textContent = issue.code === 'missingAccounts' ? '去选账号 ↓' : '去修改 ↓';
        }
      } else {
        banner.className = 'schedule-banner is-info';
        textEl.textContent = '巡检配置已修改但尚未保存，点击「立即保存」生效（未保存修改在离开页面后会丢失）。';
        actionBtn.hidden = false;
        actionBtn.textContent = '立即保存';
        actionBtn.dataset.action = 'save';
      }
      return;
    }

    if (scheduleFeedback?.kind === 'success') {
      banner.hidden = false;
      banner.className = 'schedule-banner is-success';
      textEl.textContent = scheduleFeedback.text;
      return;
    }

    banner.hidden = true;
  }

  function updateButtons() {
    const m = readTarget(manual);
    const s = readTarget(planned);
    const enabled = $('enabled').checked;
    const persistedEnabled = Boolean(schedule?.settings?.enabled);
    const dailyTimes = readDailyTimes();
    const dupTimes = scheduleMode === 'daily' && hasDuplicateDailyTimes(dailyTimes);

    $('manual-cost').textContent = `预计调用：${describeCost(m)}`;
    if (!enabled) {
      $('schedule-cost').textContent = '定时巡检已关闭，不会自动发起调用。';
    } else if (dupTimes) {
      $('schedule-cost').textContent = '存在重复的每日执行时间点，请调整后再保存。';
    } else if (scheduleMode === 'daily') {
      const tz = Number(scheduleTzSelect.getValue()) || 0;
      const sorted = [...dailyTimes].sort().join('、');
      $('schedule-cost').textContent = `每日 ${sorted} (${formatUtcOffset(tz)}) 执行 · 每次 ${describeCost(s)}`;
    } else {
      const hours = Number(scheduleHoursSelect.getValue()) || 24;
      const periodText = hours === 168 ? '每 7 天' : `每 ${hours} 小时`;
      $('schedule-cost').textContent = `${periodText} 执行 · 每次 ${describeCost(s)}`;
    }

    $('submit-run').disabled = busy || !catalog.detectorReady || !isTargetUsable(m);
    $('enabled').disabled = busy || !schedule;
    $('save').disabled = busy || !schedule;
    $('save').textContent = enabled && !persistedEnabled ? '保存并开启定时巡检' : '保存巡检设置';
    $('auto-disable-on-suspect').disabled = busy || !schedule;
    renderScheduleState();
  }

  function renderPolicyBadge(enabled) {
    const badge = $('policy-badge');
    badge.textContent = enabled ? '已开启' : '已关闭';
    badge.className = enabled ? 'z-tag z-tag-xs z-tag-warning' : 'z-tag z-tag-xs';
  }

  function applySchedule(data) {
    schedule = data;
    const cfg = data.settings || {};
    $('enabled').checked = Boolean(cfg.enabled);
    $('auto-disable-on-suspect').checked = Boolean(cfg.autoDisableOnSuspect);
    renderPolicyBadge(Boolean(cfg.autoDisableOnSuspect));
    scheduleHoursSelect.setValue(String(cfg.hours || 24));
    const offset = cfg.mode === 'daily' || cfg.utcOffsetMinutes ? (cfg.utcOffsetMinutes || 0) : browserOffsetMinutes;
    scheduleTzSelect.setOptions(buildTimezoneOptions(offset), true);
    scheduleTzSelect.setValue(String(offset));
    renderDailySlots(cfg.dailyTimes && cfg.dailyTimes.length ? cfg.dailyTimes : ['09:00']);
    setScheduleMode(cfg.mode === 'daily' ? 'daily' : 'interval', true);
    applyTarget(planned, cfg.target || { clientKeyId: '', model: '', effort: '', accountIds: [] });
    scheduleBaseline = readScheduleFormSnapshot();
    $('schedule-notice').className = 'notice';
    $('schedule-notice').textContent = data.lastSkip ? `上次巡检跳过原因：${reasonText(data.lastSkip)}` : '';
    renderSummary();
    updateButtons();
  }

  /* ---------------- KPI Summary ---------------- */
  function renderSummary() {
    const st = accountStatuses?.summary;
    if (st) {
      const healthy = st.match + st.ranked;
      $('kpi-accounts').textContent = `${healthy} 正常 / ${st.total} 账号`;
      const parts = [];
      if (st.running + st.queued > 0) parts.push(`检测中 ${st.running} · 排队 ${st.queued}`);
      if (st.suspect > 0) parts.push(`疑似异常 ${st.suspect}`);
      const attn = st.failed + st.insufficient + st.interrupted;
      if (attn > 0) parts.push(`失败/不足 ${attn}`);
      if (st.untested > 0) parts.push(`未检测 ${st.untested}`);
      $('kpi-accounts-sub').textContent = parts.length ? parts.join(' · ') : '全部账号状态正常';

      const statusBadge = $('tab-status-badge');
      statusBadge.textContent = String(st.total);
      statusBadge.classList.toggle('is-alert', st.suspect > 0);
      statusBadge.classList.toggle('is-active-work', st.suspect === 0 && (st.running + st.queued) > 0);
    }

    const sum = runsData?.summary;
    if (sum) {
      const active = sum.running + sum.queued;
      if (sum.running > 0) {
        $('queue-count').textContent = `${sum.running} 执行中 · ${sum.queued} 排队`;
      } else if (sum.queued > 0) {
        $('queue-count').textContent = `${sum.queued} 项排队中`;
      } else {
        $('queue-count').textContent = '空闲';
      }
      $('live-indicator').classList.toggle('is-busy', active > 0);
      $('history-summary').textContent = `共 ${sum.total} 条`;
      $('history-sub').textContent = `匹配 ${sum.match} · 参考 ${sum.ranked} · 疑似 ${sum.suspect} · 失败 ${sum.failed}`;
      $('tab-records-badge').textContent = String(sum.total);
      $('tab-records-badge').classList.toggle('is-active-work', active > 0);
      $('cancel-pending').hidden = sum.queued === 0;
      $('cancel-pending').textContent = `取消排队任务（${sum.queued}）`;
    }

    if (schedule) {
      const guardSuffix = schedule.settings.autoDisableOnSuspect ? ' · 异常自动停用已开' : '';
      if (schedule.settings.enabled) {
        $('due').textContent = describeScheduleShort(schedule.settings);
        $('due-sub').textContent = `下次执行：${localDate(schedule.nextDueAt)}${guardSuffix}`;
      } else {
        $('due').textContent = '已关闭';
        $('due-sub').textContent = `支持按间隔或每日固定时间${guardSuffix}`;
      }
      $('last-skip').hidden = !schedule.lastSkip;
      $('last-skip').textContent = schedule.lastSkip ? `上次定时巡检未入队：${reasonText(schedule.lastSkip)}` : '';
    }
  }

  /* ---------------- Tab 1: Account Live Status View ---------------- */
  function renderStatusPills(summary) {
    if (!summary) return;
    const attention = (summary.failed || 0) + (summary.insufficient || 0) + (summary.interrupted || 0) + (summary.skipped || 0);
    const counts = {
      total: summary.total || 0,
      running: summary.running || 0,
      queued: summary.queued || 0,
      match: summary.match || 0,
      ranked: summary.ranked || 0,
      suspect: summary.suspect || 0,
      attention,
      untested: summary.untested || 0,
    };
    for (const pill of $('status-pills').querySelectorAll('.z-pill')) {
      const target = pill.dataset.status || '';
      pill.classList.toggle('is-active', statusFilter.verdict === target);
      const span = pill.querySelector('[data-status-count]');
      if (span) span.textContent = String(counts[span.dataset.statusCount] ?? 0);
    }
  }

  function syncStatusPlanOptions(accounts) {
    const plans = [...new Set(accounts.map(a => a.planType).filter(Boolean))].sort();
    const opts = [{ value: '', label: '全部套餐' }, ...plans.map(p => ({ value: p, label: p.toUpperCase() }))];
    if (statusFilter.plan && !plans.includes(statusFilter.plan)) {
      opts.push({ value: statusFilter.plan, label: statusFilter.plan.toUpperCase() });
    }
    statusPlanSelect.setOptions(opts, true);
    statusPlanSelect.setValue(statusFilter.plan);
  }

  function renderLatestCheckCell(acc, nowMs) {
    const td = make('td');
    const last = acc.lastCheck;
    if (!last) {
      td.append(make('span', '尚未执行过指纹检测', 'muted'));
      return td;
    }

    const headLine = make('div', null, 'cell-meta');
    headLine.style.marginTop = '0';
    headLine.append(
      make('span', last.model || '—', 'cell-main'),
      make('span', effortLabel(last.effort), 'z-tag z-tag-xs'),
      make('span', last.scheduled ? '定时' : '手动', `z-tag z-tag-xs${last.scheduled ? ' z-tag-info' : ''}`),
    );
    if (acc.activeTask) {
      headLine.append(makeVerdictTag(last.verdict, `上次：${verdictLabels[last.verdict] || last.verdict}`, 'z-tag-xs'));
    }
    td.append(headLine);

    const candidates = last.candidates || [];
    if (candidates.length) {
      const first = candidates[0];
      const top = make('div', null, 'rank-top');
      top.style.marginTop = '4px';
      top.append(document.createTextNode(`#1 ${first.label}`));
      if (first.probabilityBps != null) {
        top.append(make('span', formatProb(first.probabilityBps), 'rank-prob'));
      }
      if (last.roundCount === 2) {
        top.append(make('span', ' · 2轮复测', 'muted'));
      }
      td.append(top);
      if (candidates.length > 1) {
        const rest = candidates.slice(1).map((c, idx) => `#${idx + 2} ${c.label}${c.probabilityBps != null ? ` ${formatProb(c.probabilityBps)}` : ''}`).join(' · ');
        td.append(make('div', rest, 'rank-others'));
      }
    } else if (last.reason) {
      td.append(make('div', reasonText(last.reason), 'detail'));
    }

    if (last.errorDetail && last.verdict !== 'match' && last.verdict !== 'ranked') {
      td.append(make('div', last.errorDetail, 'detail mono'));
    }

    if (last.autoDisabled === true) {
      const box = make('div', null, 'auto-disable-block');
      box.append(make('span', '已自动停用并更新备注', 'z-tag z-tag-xs z-tag-warning'));
      if (last.autoDisableNote) {
        box.append(make('div', last.autoDisableNote, 'detail'));
      }
      td.append(box);
    } else if (last.autoDisabled === false && last.autoDisableError) {
      const box = make('div', null, 'auto-disable-block');
      box.append(make('span', '自动停用失败', 'z-tag z-tag-xs z-tag-danger'));
      box.append(make('div', last.autoDisableError, 'detail mono'));
      td.append(box);
    }

    if (acc.lastScored?.candidates?.length) {
      const prevTop = acc.lastScored.candidates[0];
      const prevLine = make('div',
        `上次有效指纹：${verdictLabels[acc.lastScored.verdict] || acc.lastScored.verdict} · #1 ${prevTop.label}${prevTop.probabilityBps != null ? ` (${formatProb(prevTop.probabilityBps)})` : ''} · ${relativeTime(acc.lastScored.endedAt, nowMs)}`,
        'detail',
      );
      td.append(prevLine);
    }
    return td;
  }

  async function triggerQuickDetect(acc) {
    const current = readTarget(manual);
    if (!catalog.detectorReady || !current.clientKeyId || !current.model || !current.effort) {
      manual.selectedIds.clear();
      manual.selectedIds.add(acc.accountId);
      renderAccountList(manual);
      updateButtons();
      setTab('detect');
      $('manual-notice').textContent = `已为你预选账号 ${acc.display}，请选择测试 Client Key 与模型后点击「开始检测」。`;
      return;
    }
    busy = true;
    updateButtons();
    $('status-notice').textContent = `正在为 ${acc.display} 提交 ${current.model} / ${effortLabel(current.effort)} 检测任务…`;
    try {
      const bytes = crypto.getRandomValues(new Uint8Array(16));
      const runId = `m-${Array.from(bytes, b => b.toString(16).padStart(2, '0')).join('')}`;
      await request('POST', 'runs', {
        runId,
        target: {
          clientKeyId: current.clientKeyId,
          model: current.model,
          effort: current.effort,
          accountIds: [acc.accountId],
        },
      });
      $('status-notice').textContent = `已将 ${acc.display}（${current.model} / ${effortLabel(current.effort)}）加入执行队列。`;
      await refresh(false);
    } catch (error) {
      $('status-notice').textContent = `提交失败：${reasonText(error.message)}`;
    } finally {
      busy = false;
      updateButtons();
    }
  }

  function jumpToAccountHistory(acc) {
    queryState.accountId = acc.accountId;
    queryState.accountLabel = acc.display;
    queryState.page = 1;
    setTab('records');
  }

  function renderAccountStatusView() {
    if (!accountStatuses) return;
    const nowMs = accountStatuses.generatedAt || Date.now();
    const list = accountStatuses.accounts || [];
    renderStatusPills(accountStatuses.summary);
    syncStatusPlanOptions(list);

    const needle = statusFilter.search.trim().toLowerCase();
    const filtered = list.filter(acc => {
      if (statusFilter.verdict) {
        if (statusFilter.verdict === 'attention') {
          if (!['failed', 'insufficient', 'interrupted', 'skipped'].includes(acc.liveStatus)) return false;
        } else if (acc.liveStatus !== statusFilter.verdict) {
          return false;
        }
      }
      if (statusFilter.plan && (acc.planType || '') !== statusFilter.plan) return false;
      if (needle) {
        const hit =
          (acc.display && acc.display.toLowerCase().includes(needle)) ||
          (acc.email && acc.email.toLowerCase().includes(needle)) ||
          (acc.name && acc.name.toLowerCase().includes(needle)) ||
          acc.accountId.toLowerCase().includes(needle) ||
          (acc.lastCheck?.model && acc.lastCheck.model.toLowerCase().includes(needle)) ||
          (acc.activeTask?.model && acc.activeTask.model.toLowerCase().includes(needle));
        if (!hit) return false;
      }
      return true;
    });

    filtered.sort((a, b) => {
      if (statusFilter.sort === 'recent') {
        const at = a.activeTask ? Number.MAX_SAFE_INTEGER : (a.lastCheck?.endedAt || 0);
        const bt = b.activeTask ? Number.MAX_SAFE_INTEGER : (b.lastCheck?.endedAt || 0);
        if (bt !== at) return bt - at;
      } else if (statusFilter.sort === 'priority') {
        const pa = statusPriority[a.liveStatus] ?? 99;
        const pb = statusPriority[b.liveStatus] ?? 99;
        if (pa !== pb) return pa - pb;
        const at = a.lastCheck?.endedAt || 0;
        const bt = b.lastCheck?.endedAt || 0;
        if (bt !== at) return bt - at;
      }
      return (a.display || a.accountId).toLowerCase().localeCompare((b.display || b.accountId).toLowerCase());
    });

    const hasFilter = Boolean(statusFilter.verdict || statusFilter.search || statusFilter.plan || statusFilter.sort !== 'priority');
    $('status-reset').disabled = !hasFilter;

    const rows = filtered.map(acc => {
      const tr = make('tr');

      // Account column
      const accTd = make('td');
      accTd.append(make('div', acc.display, 'cell-main'));
      const meta = make('div', null, 'cell-meta');
      if (acc.planType) meta.append(make('span', acc.planType.toUpperCase(), 'z-tag z-tag-xs z-tag-primary'));
      if (acc.name && acc.name !== acc.display && acc.name !== acc.accountId) {
        meta.append(make('span', acc.name, 'z-tag z-tag-xs'));
      }
      if (acc.credentialState) {
        const isReady = acc.credentialState.toLowerCase() === 'ready';
        meta.append(make(
          'span',
          credentialStateLabels[acc.credentialState.toLowerCase()] || acc.credentialState,
          `z-tag z-tag-xs${isReady ? ' z-tag-success' : ' z-tag-warning'}`,
        ));
      }
      if (!acc.enabled) {
        meta.append(make('span', '宿主已停用', 'z-tag z-tag-xs z-tag-danger'));
      }
      if (acc.display !== acc.accountId) {
        meta.append(make('span', acc.accountId, 'mono'));
      }
      accTd.append(meta);
      accTd.title = acc.accountId;

      // Live status column
      const stTd = make('td');
      if (acc.activeTask) {
        const t = acc.activeTask;
        const label = t.status === 'running'
          ? `检测中 · 第 ${t.round} 轮`
          : `排队中 #${t.queuePosition || 1}`;
        stTd.append(makeVerdictTag(t.status, label));
        stTd.append(make('div', `${t.model} / ${effortLabel(t.effort)} · ${t.scheduled ? '定时' : '手动'}`, 'detail'));
      } else {
        stTd.append(makeVerdictTag(acc.liveStatus));
      }
      if ((acc.recentVerdicts || []).length) {
        const trail = make('div', null, 'verdict-trail');
        trail.title = `近 ${acc.recentVerdicts.length} 次记录：${acc.recentVerdicts.map(v => verdictLabels[v] || v).join(' ← ')}`;
        for (const v of [...acc.recentVerdicts].reverse()) {
          trail.append(make('span', null, `trail-dot ${v}`));
        }
        trail.append(make('span', `共 ${acc.totalChecks} 次`, 'trail-count'));
        stTd.append(trail);
      }

      // Latest check column
      const latestTd = renderLatestCheckCell(acc, nowMs);

      // Time / duration column
      const timeTd = make('td');
      if (acc.activeTask) {
        timeTd.append(make('div', `入队于 ${relativeTime(acc.activeTask.queuedAt, nowMs)}`, 'cell-main'));
        timeTd.append(make('div', localDate(acc.activeTask.queuedAt), 'detail'));
      } else if (acc.lastCheck) {
        timeTd.append(make('div', relativeTime(acc.lastCheck.endedAt, nowMs), 'cell-main'));
        timeTd.append(make('div', localDate(acc.lastCheck.endedAt), 'detail'));
        const dur = formatDuration(acc.lastCheck.durationMs);
        const probes = acc.lastCheck.probesTotal ? `探针 ${acc.lastCheck.probesOk}/${acc.lastCheck.probesTotal}` : '';
        const perf = [dur, probes].filter(Boolean).join(' · ');
        if (perf) timeTd.append(make('div', perf, 'detail mono'));
      } else {
        timeTd.append(make('span', '—', 'muted'));
      }

      // Actions column
      const actTd = make('td');
      const actions = make('div', null, 'row-actions');
      const detectBtn = make('button', acc.activeTask ? '队列中' : '检测', 'z-btn z-btn-primary-plain z-btn-xs');
      detectBtn.type = 'button';
      detectBtn.disabled = busy || !acc.enabled || Boolean(acc.activeTask) || !catalog.detectorReady;
      detectBtn.title = !acc.enabled
        ? '该账号已在宿主中停用'
        : acc.activeTask
          ? '该账号已有待执行或执行中的检测任务'
          : '立即对该账号发起指纹检测';
      detectBtn.addEventListener('click', () => triggerQuickDetect(acc));
      actions.append(detectBtn);

      const histBtn = make('button', '历史', 'z-btn z-btn-plain z-btn-xs');
      histBtn.type = 'button';
      histBtn.disabled = !acc.totalChecks && !acc.activeTask;
      histBtn.addEventListener('click', () => jumpToAccountHistory(acc));
      actions.append(histBtn);

      if (acc.lastCheck) {
        const logBtn = make('button', '日志', 'z-btn z-btn-plain z-btn-xs');
        logBtn.type = 'button';
        logBtn.addEventListener('click', () => {
          setTab('records');
          openLogs(acc.lastCheck.runId);
        });
        actions.append(logBtn);
      }
      actTd.append(actions);

      tr.append(accTd, stTd, latestTd, timeTd, actTd);
      return tr;
    });

    $('status-rows').replaceChildren(...rows);
    $('status-table-wrap').hidden = rows.length === 0;
    $('status-empty').hidden = rows.length > 0;
    if (!rows.length) {
      $('status-empty').textContent = list.length ? '没有符合筛选条件的账号。' : '当前暂无启用的 OpenAI OAuth 账号。';
    }
  }

  async function fetchAccountStatuses() {
    try {
      const data = await request('GET', 'accounts/status');
      applyAccountStatuses(data);
    } catch (error) {
      $('status-notice').textContent = reasonText(error.message);
    }
  }

  function applyAccountStatuses(data) {
    accountStatuses = data;
    statusByAccountId = new Map((data.accounts || []).map(a => [a.accountId, a]));
    $('live-updated').textContent = `同步于 ${shortTime.format(new Date(data.generatedAt || Date.now()))}`;
    renderSummary();
    renderAccountStatusView();
    renderAccountList(manual);
    renderAccountList(planned);
  }

  /* ---------------- Tab 3: Task Records ---------------- */
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
    for (const pill of $('quick-pills').querySelectorAll('.z-pill')) {
      const verdict = pill.dataset.verdict || '';
      pill.classList.toggle('is-active', queryState.verdict === verdict);
      const countEl = pill.querySelector('[data-count]');
      if (countEl) countEl.textContent = String(map[countEl.dataset.count] ?? 0);
    }
  }

  function syncFilterOptions(filters) {
    const modelSet = new Set([...(filters?.models || []), ...catalog.models.map(m => m.id)]);
    if (queryState.model) modelSet.add(queryState.model);
    const models = [...modelSet].sort();
    filterModelSelect.setOptions([{ value: '', label: '全部模型' }, ...models.map(m => ({ value: m, label: m }))], true);
    filterModelSelect.setValue(queryState.model);

    const effortSet = new Set(filters?.efforts || []);
    for (const m of catalog.models) {
      for (const e of m.efforts || []) effortSet.add(e);
    }
    if (queryState.effort) effortSet.add(queryState.effort);
    const efforts = [...effortSet];
    filterEffortSelect.setOptions([{ value: '', label: '全部档位' }, ...efforts.map(e => ({ value: e, label: effortLabel(e) }))], true);
    filterEffortSelect.setValue(queryState.effort);

    $('account-filter-banner').hidden = !queryState.accountId;
    if (queryState.accountId) {
      $('account-filter-label').textContent = `当前仅看账号：${queryState.accountLabel || queryState.accountId}`;
    }

    const hasFilter = Boolean(
      queryState.search || queryState.source || queryState.verdict || queryState.model || queryState.effort || queryState.accountId
    );
    $('reset-filters').disabled = !hasFilter;
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
    pageSizeSelect.setValue(String(pageSize));

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
      if (prev && p - prev > 1) buttons.push(make('span', '…', 'page-ellipsis'));
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
      meta.append(make('span', item.scheduled ? '定时巡检' : '手动检测', `z-tag z-tag-xs${item.scheduled ? ' z-tag-info' : ''}`));
      if (item.accountName && item.accountName !== primary && item.accountName !== item.accountId) {
        meta.append(make('span', item.accountName, 'z-tag z-tag-xs'));
      }
      if (primary !== item.accountId) {
        meta.append(make('span', item.accountId, 'mono'));
      }
      accountTd.append(meta);
      accountTd.title = item.accountId;

      const modelTd = make('td');
      modelTd.append(make('div', item.model || '—', 'cell-main'));
      if (item.effort) {
        const effMeta = make('div', null, 'cell-meta');
        effMeta.append(make('span', effortLabel(item.effort), 'z-tag z-tag-xs'));
        modelTd.append(effMeta);
      }

      const statusTd = make('td');
      const code = item.status || item.outcome?.verdict || 'queued';
      statusTd.append(makeVerdictTag(code));
      if (item.outcome?.reason) {
        statusTd.append(make('div', reasonText(item.outcome.reason), 'detail'));
      } else if (item.kind === 'queue') {
        statusTd.append(make('div', item.round === 2 ? '第 2 轮复测中' : '第 1 轮检测', 'detail'));
      }
      if (item.diagnostics) {
        const d = item.diagnostics;
        const okProbes = (d.probes || []).filter(p => p.ok).length;
        const totalProbes = (d.probes || []).length;
        const sec = formatDuration(d.durationMs);
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
        if (d.autoDisabled === true) {
          const box = make('div', null, 'auto-disable-block');
          box.append(make('span', '已自动停用并更新备注', 'z-tag z-tag-xs z-tag-warning'));
          if (d.autoDisableNote) {
            box.append(make('div', d.autoDisableNote, 'detail'));
          }
          statusTd.append(box);
        } else if (d.autoDisabled === false && d.autoDisableError) {
          const box = make('div', null, 'auto-disable-block');
          box.append(make('span', '自动停用失败', 'z-tag z-tag-xs z-tag-danger'));
          box.append(make('div', d.autoDisableError, 'detail mono'));
          statusTd.append(box);
        }
      }

      const timeTd = make('td');
      timeTd.append(make('div', localDate(item.endedAt || item.queuedAt), 'cell-main'));
      timeTd.append(make('div', item.runId, 'detail mono'));
      timeTd.title = `任务 ${item.runId}${item.pluginVersion ? ` · 插件 ${item.pluginVersion}` : ''}`;

      const actionTd = make('td');
      const logBtn = make('button', '日志', 'z-btn z-btn-plain z-btn-xs');
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
      const filtered = Boolean(queryState.search || queryState.source || queryState.verdict || queryState.model || queryState.effort || queryState.accountId);
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
        accountId: queryState.accountId,
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
      if (manual.key.getValue()) catQuery.clientKeyId = manual.key.getValue();

      const epochBefore = scheduleEpoch;
      const [nextCatalog, nextStatus, nextRuns, nextSchedule] = await Promise.all([
        full ? request('GET', 'catalog', null, catQuery) : catalog,
        request('GET', 'accounts/status'),
        request('GET', 'runs', null, {
          page: queryState.page,
          pageSize: queryState.pageSize,
          search: queryState.search,
          source: queryState.source,
          verdict: queryState.verdict,
          model: queryState.model,
          effort: queryState.effort,
          accountId: queryState.accountId,
        }),
        request('GET', 'schedule'),
      ]);

      catalog = nextCatalog;
      const scheduleFresh = epochBefore === scheduleEpoch;
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
        if (scheduleFresh) {
          clearScheduleFeedback();
          applySchedule(nextSchedule);
        }
      } else if (scheduleFresh) {
        const remoteChanged = schedule && JSON.stringify(schedule.settings) !== JSON.stringify(nextSchedule.settings);
        if (remoteChanged && !isScheduleDirty()) {
          applySchedule(nextSchedule);
        } else {
          schedule = nextSchedule;
        }
      }
      applyAccountStatuses(nextStatus);
      renderRuns(nextRuns);
      updateButtons();
    } catch (error) {
      if (full) status(reasonText(error.message), true);
    } finally {
      polling = false;
      $('refresh').disabled = false;
    }
  }

  /* ---------------- Schedule Persistence & Event Bindings ---------------- */
  async function persistScheduleSettings(settings, { syncForm = true, successMessage = '' } = {}) {
    busy = true;
    scheduleEpoch += 1;
    updateButtons();
    $('schedule-notice').className = 'notice';
    $('schedule-notice').textContent = '正在保存巡检设置…';
    try {
      const updated = await request('PUT', 'schedule', { settings, previous: schedule.settings });
      scheduleEpoch += 1;
      if (syncForm) {
        applySchedule(updated);
      } else {
        schedule = updated;
        if (scheduleBaseline) {
          scheduleBaseline.enabled = Boolean(updated.settings?.enabled);
        }
        $('enabled').checked = Boolean(updated.settings?.enabled);
        renderSummary();
      }
      const msg = successMessage || (updated.settings?.enabled
        ? `定时巡检已保存并启用（${describeScheduleShort(updated.settings)}，下次执行：${localDate(updated.nextDueAt)}）。`
        : '定时巡检已保存（当前关闭）。');
      scheduleFeedback = { kind: 'success', text: msg };
      $('schedule-notice').className = 'notice is-success';
      $('schedule-notice').textContent = msg;
      await refresh(false);
      return true;
    } catch (error) {
      const msg = `保存失败：${reasonText(error.message)}`;
      scheduleFeedback = { kind: 'error', text: msg };
      $('schedule-notice').className = 'notice is-error';
      $('schedule-notice').textContent = msg;
      return false;
    } finally {
      busy = false;
      updateButtons();
    }
  }

  async function saveScheduleFromForm() {
    if (!schedule || busy) return false;
    const enabled = $('enabled').checked;
    const issue = validateScheduleForm(enabled);
    if (issue) {
      scheduleFeedback = { kind: 'error', text: issue.message };
      $('schedule-notice').className = 'notice is-error';
      $('schedule-notice').textContent = issue.message;
      updateButtons();
      highlightScheduleTarget(issue.focusEl);
      return false;
    }
    const selected = readTarget(planned);
    const dailyTimes = [...readDailyTimes()].sort();
    const settings = {
      enabled,
      mode: scheduleMode,
      hours: Number(scheduleHoursSelect.getValue()) || 24,
      dailyTimes,
      utcOffsetMinutes: Number(scheduleTzSelect.getValue()) || 0,
      autoDisableOnSuspect: $('auto-disable-on-suspect').checked,
      target: isTargetUsable(selected) ? selected : schedule.settings.target,
    };
    return persistScheduleSettings(settings, { syncForm: true });
  }

  $('enabled').addEventListener('change', async () => {
    if (!schedule || busy) return;
    clearScheduleFeedback();
    const nextEnabled = $('enabled').checked;
    const persistedEnabled = Boolean(schedule.settings?.enabled);

    if (nextEnabled) {
      const issue = validateScheduleForm(true);
      if (!issue) {
        const ok = await saveScheduleFromForm();
        if (!ok) {
          $('enabled').checked = persistedEnabled;
          updateButtons();
        }
        return;
      }
      $('schedule-notice').className = 'notice';
      $('schedule-notice').textContent = `定时巡检尚未生效：${issue.message}`;
      updateButtons();
      return;
    }

    if (persistedEnabled) {
      const settings = {
        ...schedule.settings,
        enabled: false,
        autoDisableOnSuspect: $('auto-disable-on-suspect').checked,
      };
      const ok = await persistScheduleSettings(settings, {
        syncForm: false,
        successMessage: '定时巡检已关闭（已保留原巡检配置，再次开启即可恢复）。',
      });
      if (!ok) {
        $('enabled').checked = persistedEnabled;
        updateButtons();
      }
      return;
    }

    $('schedule-notice').className = 'notice';
    $('schedule-notice').textContent = '';
    updateButtons();
  });

  $('schedule-banner-locate').addEventListener('click', () => {
    const issue = validateScheduleForm($('enabled').checked);
    if (issue?.focusEl) highlightScheduleTarget(issue.focusEl);
  });

  $('schedule-banner-action').addEventListener('click', async () => {
    if (!schedule || busy) return;
    const action = $('schedule-banner-action').dataset.action;
    if (action === 'select-all-and-enable') {
      planned.selectedIds.clear();
      for (const a of catalog.accounts) {
        if (planned.selectedIds.size >= catalog.maxBatch) break;
        planned.selectedIds.add(a.id);
      }
      renderAccountList(planned);
      clearScheduleFeedback();
      updateButtons();
      await saveScheduleFromForm();
      return;
    }
    if (action === 'save') {
      await saveScheduleFromForm();
    }
  });

  $('auto-disable-on-suspect').addEventListener('change', async () => {
    if (!schedule || busy) return;
    const nextValue = $('auto-disable-on-suspect').checked;
    const prevSettings = schedule.settings;
    const tzOffset = prevSettings.mode === 'daily' || prevSettings.utcOffsetMinutes
      ? (prevSettings.utcOffsetMinutes || 0)
      : browserOffsetMinutes;
    const settings = {
      ...prevSettings,
      utcOffsetMinutes: tzOffset,
      autoDisableOnSuspect: nextValue,
    };
    busy = true;
    scheduleEpoch += 1;
    renderPolicyBadge(nextValue);
    updateButtons();
    $('policy-notice').textContent = '正在保存自动停用设置…';
    try {
      const updated = await request('PUT', 'schedule', { settings, previous: prevSettings });
      scheduleEpoch += 1;
      schedule = updated;
      const enabledGuard = Boolean(updated.settings?.autoDisableOnSuspect);
      $('auto-disable-on-suspect').checked = enabledGuard;
      renderPolicyBadge(enabledGuard);
      renderSummary();
      $('policy-notice').textContent = enabledGuard
        ? `已开启「指纹异常自动停用」：两轮确认疑似异常的账号将自动停用并更新备注（备注时区 ${formatUtcOffset(updated.settings.utcOffsetMinutes || 0)}）。`
        : '已关闭「指纹异常自动停用」：检测仅记录结论，不会修改账号状态或备注。';
      await refresh(false);
    } catch (error) {
      $('auto-disable-on-suspect').checked = Boolean(prevSettings.autoDisableOnSuspect);
      renderPolicyBadge(Boolean(prevSettings.autoDisableOnSuspect));
      $('policy-notice').textContent = `保存失败：${reasonText(error.message)}`;
    } finally {
      busy = false;
      updateButtons();
    }
  });
  $('refresh').addEventListener('click', () => refresh(true, true));
  $('refresh-status').addEventListener('click', () => {
    $('status-notice').textContent = '';
    fetchAccountStatuses();
  });
  $('refresh-records').addEventListener('click', () => {
    $('records-notice').textContent = '';
    fetchRuns();
  });

  $('status-pills').addEventListener('click', e => {
    const pill = e.target.closest('.z-pill');
    if (!pill) return;
    statusFilter.verdict = pill.dataset.status || '';
    renderAccountStatusView();
  });

  $('status-search').addEventListener('input', () => {
    clearTimeout(statusSearchTimer);
    statusSearchTimer = setTimeout(() => {
      statusFilter.search = $('status-search').value;
      renderAccountStatusView();
    }, 180);
  });

  $('status-reset').addEventListener('click', () => {
    statusFilter.verdict = '';
    statusFilter.search = '';
    statusFilter.plan = '';
    statusFilter.sort = 'priority';
    $('status-search').value = '';
    statusPlanSelect.setValue('');
    statusSortSelect.setValue('priority');
    $('status-notice').textContent = '';
    renderAccountStatusView();
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
    await saveScheduleFromForm();
  });

  $('cancel-pending').addEventListener('click', async () => {
    busy = true;
    $('cancel-pending').disabled = true;
    try {
      const res = await request('POST', 'runs/cancel', {});
      $('records-notice').textContent = res.cancelled > 0
        ? `已取消 ${res.cancelled} 项排队任务（正在执行中的任务不受影响）。`
        : '没有可取消的排队任务。';
      await refresh(false);
    } catch (error) {
      $('records-notice').textContent = reasonText(error.message);
    } finally {
      busy = false;
      $('cancel-pending').disabled = false;
    }
  });

  $('clear-account-filter').addEventListener('click', () => {
    queryState.accountId = '';
    queryState.accountLabel = '';
    queryState.page = 1;
    fetchRuns();
  });

  $('quick-pills').addEventListener('click', e => {
    const pill = e.target.closest('.z-pill');
    if (!pill) return;
    queryState.verdict = pill.dataset.verdict || '';
    filterVerdictSelect.setValue(queryState.verdict);
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

  $('reset-filters').addEventListener('click', () => {
    queryState.search = '';
    queryState.source = '';
    queryState.verdict = '';
    queryState.model = '';
    queryState.effort = '';
    queryState.accountId = '';
    queryState.accountLabel = '';
    queryState.page = 1;
    $('filter-search').value = '';
    filterSourceSelect.setValue('');
    filterVerdictSelect.setValue('');
    filterModelSelect.setValue('');
    filterEffortSelect.setValue('');
    $('records-notice').textContent = '';
    fetchRuns();
  });

  /* ---------------- Diagnostics Log Drawer ---------------- */
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
        if (d.autoDisabled === true) {
          lines.push(`自动处置: 已自动停用并写入备注 (${d.autoDisableNote || '-'})`);
        } else if (d.autoDisabled === false) {
          lines.push(`自动处置: 自动停用失败 (${d.autoDisableError || '-'})`);
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
      if (d.autoDisabled === true) {
        addKv('异常自动停用', `已停用并更新备注 · ${d.autoDisableNote || '—'}`);
      } else if (d.autoDisabled === false) {
        addKv('异常自动停用', `失败 · ${d.autoDisableError || '—'}`);
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
    const fmt = logFormatSelect.getValue();
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
      } else if (logScopeSelect.getValue() === 'all') {
        params.scope = 'all';
      } else {
        params.search = queryState.search;
        params.source = queryState.source;
        params.verdict = queryState.verdict;
        params.model = queryState.model;
        params.effort = queryState.effort;
        params.accountId = queryState.accountId;
      }
      logBundle = await request('GET', 'logs', null, params);
      renderLogOutput();
    } catch (error) {
      $('log-notice').textContent = `采集日志失败：${reasonText(error.message)}`;
    }
  }

  function openLogs(runId = null) {
    logTargetRunId = runId || null;
    $('log-scope-host').hidden = Boolean(logTargetRunId);
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
    const isText = logFormatSelect.getValue() === 'text';
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
