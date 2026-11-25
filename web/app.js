'use strict';
(() => {
  const $ = id => document.getElementById(id);
  const bridge = window.codexProxyPlugin;
  const reasons = {
    invalidInput: '请选择档位、测试 Key 和 1–10 个账号，并检查预算是否在 3–60 之间。',
    conflict: '已有相同目标在队列中，或设置已改变。请刷新后重试。',
    queueFull: '队列已满，请稍后再试。', budget: '共享探针预算不足，本次跳过。',
    unsupportedProfile: '该模型档位尚未通过验证，不能用于正式判定。',
    detectorUnavailable: '参考库未就绪，未发送探针。', accountUnavailable: '指定账号不可用或不再是启用的 OAuth 账号。',
    accountChanged: '采样中账号凭据版本发生变化。', accessDenied: '所选 Key 无法访问这些账号或此模型。',
    rateLimited: '宿主或上游限流。', timeout: '模型调用或宿主回调超时。', hostFailure: '宿主未能完成本次操作。',
    cancelled: '宿主取消了任务。', incomplete: '回复未完整结束、被截断或不可用。', transformed: '回复被宿主中间件改写。',
    notScorable: '样本不可评分，或参考库未校准。', nearby: '目标名次接近，或头两名差距不足。',
    inconsistent: '复测结果不一致。', versionChanged: '复测前探针档位、参考库或账号凭据版本变更。',
    restarted: '上次调用未能保存结果；为避免重复消耗，未重发。', scheduleChanged: '巡检已关闭或计划已变更。', staleSlot: '旧时间槽已过期，不补跑。',
  };
  const labels = { match: '未发现指纹异常', suspect: '疑似指纹不符', insufficient: '证据不足', failed: '检测失败',
    interrupted: '中断',skipped: '已跳过',calibration: '参考排名（无判定）', running: '检测中', queued: '等待检测' };
  const stamp = new Intl.DateTimeFormat('zh-CN', { dateStyle: 'medium', timeStyle: 'short', hour12: false });
  const localDate = at => at == null ? '—' : stamp.format(new Date(at));
  const reason = code => reasons[code] || '读取失败，请刷新后重试。';
  const make = (tag, text, className) => {
    const el = document.createElement(tag);
    if (text != null) el.textContent = text;
    if (className) el.className = className;
    return el;
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
  function theme() { document.documentElement.dataset.theme = bridge.theme === 'dark' ? 'dark' : 'light'; }
  window.addEventListener('codex-proxy-themechange', theme);
  theme();

  async function request(method, path, value) {
    const res = await bridge.request({ method, path, ...(value == null ? {} : { contentType: 'application/json', body: JSON.stringify(value) }) });
    let json;
    try { json = JSON.parse(new TextDecoder().decode(res.body)); } catch { throw new Error('hostFailure'); }
    if (res.status < 200 || res.status >= 300) throw new Error(json.error || 'hostFailure');
    return json;
  }
  const initial = { accounts: [], keys: [], profiles: [], maxBatch: 10 };
  let catalog = initial;
  let schedule = null;
  let runs = null;
  let busy = false;
  let polling = false;
  let nonce = null;

  function controls(place) {
    const key = make('select');
    const profile = make('select');
    const accounts = make('div', null, 'accounts');
    const keyLabel = make('label','测试 Client Key'); keyLabel.append(key);
    const profileLabel = make('label','模型 × 推理档位'); profileLabel.append(profile);
    const fields = make('div',null,'fields'); fields.append(keyLabel, profileLabel);
    const group = make('fieldset'); group.append(make('legend','指定账号'), accounts);
    $(place).append(fields,group);
    return { key, profile, accounts, group };
  }
  const manual = controls('manual-target');
  const planned = controls('schedule-target');
  const target = controls => ({ clientKeyId: controls.key.value, profileId: controls.profile.value,
    accountIds: [...controls.accounts.querySelectorAll('input:checked')].map(el => el.value).sort() });
  const usable = t => t.clientKeyId && t.profileId && t.accountIds.length > 0 && t.accountIds.length <= catalog.maxBatch;
  const option = (value, text) => { const item = make('option', text); item.value = value; return item; };

  function refreshTarget(controls, mode, chosen = target(controls)) {
    controls.key.replaceChildren(option('', '选择已启用 Key'), ...catalog.keys.map(k => option(k.id, k.name || k.id)));
    controls.key.value = catalog.keys.some(k => k.id === chosen.clientKeyId) ? chosen.clientKeyId : '';
    const profiles = catalog.profiles.filter(p => mode === 'calibration' || p.validated);
    controls.profile.replaceChildren(option('', profiles.length ? '选择档位' : '无可用档位'), ...profiles.map(p => option(p.id, `${p.model} · ${p.effort}`)));
    controls.profile.value = profiles.some(p => p.id === chosen.profileId) ? chosen.profileId : '';
    controls.accounts.replaceChildren(...catalog.accounts.map(a => {
      const check = make('input'); check.type = 'checkbox'; check.value = a.id; check.checked = chosen.accountIds.includes(a.id);
      const row = make('label', null, 'account'); row.append(check, document.createTextNode(a.name || a.id));
      row.title = a.id; return row;
    }));
    if (!catalog.accounts.length) controls.accounts.append(make('div','暂无启用的 OpenAI OAuth 账号','account muted'));
  }

  function estimate(t, mode) {
    const n = t.accountIds.length * 3;
    return `${n}${mode === 'detect' && n ? `–${n * 2}` : ''} 次模型调用（按宿主 Key 规则计费）`;
  }
  function updateButtons() {
    const m = target(manual); const s = target(planned);
    $('manual-cost').textContent = `预计调用：${estimate(m,$('mode').value)}`;
    $('schedule-cost').textContent = $('enabled').checked ? `每批预计：${estimate(s, 'detect')}` : '巡检关闭，不会自动发起调用。';
    $('submit-run').disabled = busy || !catalog.detectorReady || !usable(m) || !runs || runs.remaining < 3;
    $('enabled').disabled = !catalog.profiles.some(p => p.validated) && !$('enabled').checked;
    $('save').disabled = busy || !schedule || ($('enabled').checked && (!catalog.detectorReady || !usable(s)));
    for (const c of [manual, planned]) {
      const count = target(c).accountIds.length;
      c.group.querySelector('legend').textContent = `指定账号（${count}/${catalog.maxBatch}）`;
      for (const input of c.accounts.querySelectorAll('input')) input.disabled = !input.checked && count >= catalog.maxBatch;
    }
  }
  function useSchedule(data) {
    schedule = data; $('enabled').checked = data.settings.enabled; $('hours').value = data.settings.hours;
    $('budget').value = data.settings.probeBudget;
    refreshTarget(planned, 'detect', data.settings.target || { clientKeyId:'',profileId:'',accountIds:[] });
    $('schedule-notice').textContent = data.lastSkip ? `上次巡检：${reason(data.lastSkip)}` : '';
  }
  const profileText = id => {
    const p = catalog.profiles.find(item => item.id === id);
    return p ? `${p.model} · ${p.effort}` : id;
  };
  function rankings(item) {
    const cell = make('td');
    const summaries = [item.firstRound, item.outcome.lastRound].filter(Boolean);
    for (const [i, round] of summaries.entries()) {
      const candidates = round.ranking.candidates.map(c => c.label).join(' / ');
      const row = make('div', `${summaries.length > 1 ? `第 ${i + 1} 轮：` : ''}${candidates}`);
      row.title = `参考排名差 ${round.topMarginBps / 100} 个百分点 · ${round.detectorRevision || round.stamp}`;
      cell.append(row);
    }
    if (!summaries.length) cell.textContent = '—';
    return cell;
  }
  function renderRuns(next, currentSchedule) {
    runs = next;
    $('remaining').textContent = `${next.remaining} / ${next.budget}`;
    $('queue-count').textContent = `${next.queue.length} 项`;
    $('due').textContent = currentSchedule.settings.enabled ? localDate(currentSchedule.nextDueAt) : '已关闭';
    $('last-skip').hidden = !currentSchedule.lastSkip;
    $('last-skip').textContent = currentSchedule.lastSkip ? `上次巡检未入队：${reason(currentSchedule.lastSkip)}` : '';
    const rows = [...next.queue,...next.history].map(item => {
      const tr = make('tr');
      const name = catalog.accounts.find(a => a.id === item.accountId)?.name || item.accountId;
      const account = make('td',name); account.title = item.accountId;
      account.append(make('div',item.scheduled ? '定时巡检' : '手动任务','detail'));
      const prof = make('td',profileText(item.profileId));
      const verdict = item.outcome?.verdict || (item.running ? 'running' : 'queued');
      const state = make('td'); state.append(make('span',labels[verdict] || verdict, `badge ${verdict}`));
      if (item.outcome?.reason) state.append(make('div',reason(item.outcome.reason),'detail'));
      else if (!item.outcome) state.append(make('div',`第 ${item.round} 轮`,'detail'));
      const at = make('td',localDate(item.endedAt || item.queuedAt));
      at.title = `任务 ${item.runId}${item.pluginVersion ? ` · 插件 ${item.pluginVersion}` : ''}`;
      tr.append(account,prof,state,item.outcome ? rankings(item) : make('td','—'),at);
      return tr;
    });
    $('rows').replaceChildren(...rows);
    $('table-wrap').hidden = !rows.length;
    $('empty').hidden = !!rows.length;
    $('empty').textContent = '还没有检测记录。';
    updateButtons();
  }
  async function refresh(full) {
    if (polling) return;
    polling = true;
    if (full) $('refresh').disabled = true;
    try {
      const [nextCatalog,nextRuns,nextSchedule] = await Promise.all([
        full ? request('GET','catalog') : catalog, request('GET','runs'), request('GET','schedule'),
      ]);
      catalog = nextCatalog;
      if (full) {
        if (!catalog.profiles.some(p => p.validated) && catalog.profiles.length) $('mode').value = 'calibration';
        const validated = catalog.profiles.filter(p => p.validated).length;
        status(!catalog.detectorReady ? '参考库未通过加载校验；不会发送探针。' : validated
          ? `${validated} 个档位已通过验证。仅对通过验证的档位给出指纹结论。`
          : '当前没有经过验证的档位。可手动做校准采样；不可启用正式判定或巡检。', !catalog.detectorReady);
        $('version').textContent = `插件 ${catalog.pluginVersion} · 参考库 ${catalog.detectorRevision || '未加载'}`;
        refreshTarget(manual,$('mode').value);
        useSchedule(nextSchedule);
      }
      renderRuns(nextRuns,nextSchedule);
    } catch (error) { if (full) status(reason(error.message), true); }
    finally { polling = false; $('refresh').disabled = false; }
  }
  $('mode').addEventListener('change', () => { refreshTarget(manual,$('mode').value); updateButtons(); });
  $('manual').addEventListener('change',updateButtons);
  $('schedule').addEventListener('change',updateButtons);
  $('refresh').addEventListener('click',() => refresh(true));
  $('manual').addEventListener('submit', async e => {
    e.preventDefault(); if ($('submit-run').disabled) return;
    const selected = target(manual); const mode = $('mode').value;
    const signature = JSON.stringify({ target:selected, mode });
    if (!nonce || nonce.signature !== signature) {
      const bytes = crypto.getRandomValues(new Uint8Array(16));
      nonce = { signature, runId: `m-${Array.from(bytes, b => b.toString(16).padStart(2,'0')).join('')}` };
    }
    busy = true; updateButtons(); $('manual-notice').textContent = '正在加入队列…';
    try {
      const accepted = await request('POST','runs',{ target:selected, mode, runId:nonce.runId });
      nonce = null; $('manual-notice').textContent = `已加入队列。最多 ${accepted.probesMax} 次调用；等待宿主巡检执行。`;
      await refresh(false);
    } catch (error) { $('manual-notice').textContent = reason(error.message); }
    finally { busy = false; updateButtons(); }
  });
  $('schedule').addEventListener('submit', async e => {
    e.preventDefault(); if ($('save').disabled) return;
    const selected = target(planned);
    const settings = { enabled:$('enabled').checked, hours:Number($('hours').value),
      probeBudget:Number($('budget').value), target:usable(selected) ? selected : schedule.settings.target };
    if (!$('budget').checkValidity()) { $('budget').reportValidity(); return; }
    busy = true; updateButtons();
    try {
      useSchedule(await request('PUT','schedule',{ settings, previous:schedule.settings }));
      $('schedule-notice').textContent = '巡检设置已保存。';
      await refresh(false);
    } catch (error) { $('schedule-notice').textContent = reason(error.message); }
    finally { busy = false; updateButtons(); }
  });
  refresh(true);
  const timer = setInterval(() => { if (!document.hidden && !busy) refresh(!schedule); }, 5000);
  window.addEventListener('pagehide',() => clearInterval(timer),{ once:true });
})();
