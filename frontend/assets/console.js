/**
 * 控制台逻辑（F-P0-13 轮询 / US-05 模式切换 / US-06 图片管理 / US-08 离线态 / 401 引导）
 * - token 存 localStorage（rsl.tokens，支持多台电脑），无 token → 跳 /pair
 * - 轮询：页面可见 3s、隐藏降频 15s；连续 2 次失败 → 离线禁用；恢复后自动解除
 * - 任意接口 401 → 清除本机 token → 跳 /pair（US-10）
 */
(function () {
  'use strict';
  var $ = function (id) { return document.getElementById(id); };
  var API = '/api/v1';

  // ==================== token ====================

  function loadStore() {
    try { return JSON.parse(localStorage.getItem('rsl.tokens') || '{}'); }
    catch (e) { return {}; }
  }
  var token = loadStore()[location.origin];
  if (!token) { location.replace('/pair'); return; }

  // ==================== API 封装 ====================

  /** 统一请求：附带 Bearer；401 清 token 跳配对；返回 {status, body} */
  function api(method, path, body, isForm) {
    var headers = { 'Authorization': 'Bearer ' + token };
    if (!isForm && body !== undefined) { headers['Content-Type'] = 'application/json'; }
    return fetch(API + path, {
      method: method,
      headers: headers,
      body: body === undefined ? undefined : (isForm ? body : JSON.stringify(body))
    }).then(function (res) {
      if (res.status === 401) {
        // token 失效（被重置）：清除并引导重新配对
        var s = loadStore(); delete s[location.origin];
        localStorage.setItem('rsl.tokens', JSON.stringify(s));
        location.replace('/pair');
        return new Promise(function () {}); // 中断后续链
      }
      return res.json().then(function (j) { return { status: res.status, body: j }; });
    });
  }

  function toast(msg) {
    var t = $('toast');
    t.textContent = msg;
    t.classList.add('show');
    clearTimeout(t._timer);
    t._timer = setTimeout(function () { t.classList.remove('show'); }, 2600);
  }

  /** 错误码 → 可读文案（interaction-ui 4.5 展示建议） */
  function errText(body) {
    if (body && body.msg) { return body.msg; }
    return '操作失败，请稍后重试';
  }

  // ==================== 状态轮询（F-P0-13） ====================

  var pollTimer = null;
  var failCount = 0;      // 连续失败次数（≥2 判离线）
  var offline = false;
  var lastRefresh = null;

  function pollNow() {
    return api('GET', '/status').then(function (r) {
      if (r.status === 200 && r.body.code === 0) {
        failCount = 0;
        if (offline) setOffline(false);
        renderStatus(r.body.data);
      } else {
        throw new Error('status ' + r.status);
      }
    }).catch(function () {
      failCount += 1;
      if (failCount >= 2 && !offline) setOffline(true);
    }).then(function () {
      lastRefresh = Date.now();
      updatePollInfo();
      scheduleNext();
    });
  }

  function scheduleNext() {
    clearTimeout(pollTimer);
    // 页面可见 3s；切后台/不可见降为 15s（PRD 8.3 缓解浏览器省电限制）
    var delay = document.hidden ? 15000 : 3000;
    pollTimer = setTimeout(pollNow, delay);
  }
  document.addEventListener('visibilitychange', scheduleNext);

  function updatePollInfo() {
    if (!lastRefresh) { $('pollInfo').textContent = '每 3s 轮询'; return; }
    var sec = Math.max(0, Math.round((Date.now() - lastRefresh) / 1000));
    $('pollInfo').textContent = '上次刷新 ' + sec + 's 前 · 每 ' + (document.hidden ? 15 : 3) + 's 轮询';
    setTimeout(updatePollInfo, 1000);
  }

  /** 离线态切换：灰点/横幅/按钮禁用（US-08） */
  function setOffline(v) {
    offline = v;
    $('dot').classList.toggle('off', v);
    $('dotText').textContent = v ? '离线' : '在线';
    $('offlineBanner').classList.toggle('show', v);
    ['btnLock', 'btnUnlock', 'btnScreenOff', 'btnScreenOn'].forEach(function (id) { $(id).disabled = v; });
    $('segMode').querySelectorAll('button').forEach(function (b) { b.disabled = v; });
  }

  // ==================== 状态渲染 ====================

  function renderStatus(d) {
    $('devName').textContent = d.deviceName || '—';
    $('devOs').textContent = d.os || '';
    var modeText = { clock: '时钟', image: '图片', black: '黑屏' };
    $('stMode').textContent = modeText[d.lockedMode] || d.lockedMode || '—';
    // 屏幕状态：亮屏/熄屏/锁定+熄屏
    $('stDisplay').textContent = d.displayOn ? '亮屏' : (d.state === 'screen_off' ? (d.locked ? '锁定+熄屏' : '熄屏') : '熄屏');
    $('stLock').textContent = d.state === 'active' ? '正常' : '已锁定';
    $('stVer').textContent = 'v' + (d.version || '');
    var t = new Date((d.localTime || 0) * 1000);
    $('stTime').textContent = t.toLocaleTimeString();
    // 诊断视图数据
    $('dgVer').textContent = 'v' + (d.version || '');
    $('dgOs').textContent = d.os;
    var up = d.uptimeSec || 0;
    $('dgUp').textContent = Math.floor(up / 3600) + ' 小时 ' + Math.floor((up % 3600) / 60) + ' 分';
    // 系统锁提示（设置视图）
    $('cfgSysLock').textContent = d.systemLockEnabled ? '开启' : '关闭';
    $('cfgPort').textContent = location.port || '80';
    // 模式选择器高亮（默认模式）
    markSeg(d.defaultMode);
  }

  function markSeg(mode) {
    $('segMode').querySelectorAll('button').forEach(function (b) {
      b.classList.toggle('active', b.dataset.mode === mode);
    });
  }

  // ==================== 主操作（幂等，无需二次确认） ====================

  function pending(id, on) { $(id).classList.toggle('pending', on); }

  /** 通用动作：pending → 调用 → 成功立即刷新状态；失败 toast 错误文案 */
  function act(id, method, path, body, okMsg) {
    if (offline) { toast('被控端不在线'); return Promise.resolve(); }
    pending(id, true);
    return api(method, path, body).then(function (r) {
      pending(id, false);
      if (r.status === 200 && r.body.code === 0) {
        if (okMsg) toast(okMsg);
        return pollNow(); // 主操作成功后立刻额外拉一次状态
      }
      toast(errText(r.body));
    }).catch(function () {
      pending(id, false);
      toast('网络错误');
    });
  }

  $('btnLock').addEventListener('click', function () { act(this.id, 'POST', '/lock', {}, '已锁定'); });
  $('btnUnlock').addEventListener('click', function () { act(this.id, 'POST', '/unlock', {}, '已解锁'); });
  $('btnScreenOff').addEventListener('click', function () { act(this.id, 'POST', '/screen/off', {}, '已熄屏'); });
  $('btnScreenOn').addEventListener('click', function () { act(this.id, 'POST', '/screen/on', {}, '已唤醒'); });

  // 模式切换（US-05）：图片模式无图 → 40902 toast 并跳转图片管理
  $('segMode').addEventListener('click', function (e) {
    var btn = e.target.closest('button');
    if (!btn || offline) return;
    var mode = btn.dataset.mode;
    api('PUT', '/mode', { mode: mode }).then(function (r) {
      if (r.status === 200 && r.body.code === 0) {
        markSeg(mode);
        toast('已切换为' + ({ clock: '时钟', image: '图片', black: '黑屏' })[mode]);
        pollNow();
      } else if (r.body.code === 40902) {
        toast('请先上传图片');
        showView('images');
        loadImages();
      } else {
        toast(errText(r.body));
      }
    }).catch(function () { toast('网络错误'); });
  });

  // ==================== 视图切换 ====================

  function showView(name) {
    ['main', 'images', 'settings', 'diag'].forEach(function (v) {
      $('view-' + v).classList.toggle('active', v === name);
    });
  }

  $('goImages').addEventListener('click', function () { showView('images'); loadImages(); });
  $('backFromImages').addEventListener('click', function () { showView('main'); });
  $('goSettings').addEventListener('click', function () {
    showView('settings');
    api('GET', '/config').then(function (r) {
      if (r.status === 200 && r.body.code === 0) {
        var c = r.body.data;
        $('cfgMode').textContent = ({ clock: '时钟', image: '图片', black: '黑屏' })[c.defaultMode] || c.defaultMode;
        $('cfgHour').textContent = c.hour12 ? '12 小时制' : '24 小时制';
        $('cfgMask').textContent = c.imageMaskPercent + '%';
      }
    }).catch(function () {});
  });
  $('backFromSettings').addEventListener('click', function () { showView('main'); });
  $('goDiag').addEventListener('click', function () { showView('diag'); });
  $('backFromDiag').addEventListener('click', function () { showView('main'); });
  $('unpair').addEventListener('click', function () {
    var s = loadStore(); delete s[location.origin];
    localStorage.setItem('rsl.tokens', JSON.stringify(s));
    location.replace('/pair');
  });
  $('refreshNow').addEventListener('click', pollNow);

  // ==================== 图片管理（US-06 / F-P0-12） ====================

  var objectUrls = [];
  function loadImages() {
    api('GET', '/images').then(function (r) {
      if (r.status !== 200 || r.body.code !== 0) { toast(errText(r.body)); return; }
      var data = r.body.data;
      var grid = $('imgGrid');
      objectUrls.forEach(function (u) { URL.revokeObjectURL(u); });
      objectUrls = [];
      grid.innerHTML = '';
      $('imgEmpty').style.display = data.images.length ? 'none' : 'block';
      data.images.forEach(function (m) {
        var item = document.createElement('div');
        item.className = 'img-item';
        // 缩略图：带鉴权头 fetch → blob → objectURL（token 不出现在 URL 中）
        var img = document.createElement('img');
        img.alt = m.fileName;
        img.loading = 'lazy';
        fetch(API + '/images/' + m.id + '/raw', { headers: { 'Authorization': 'Bearer ' + token } })
          .then(function (res) { return res.ok ? res.blob() : null; })
          .then(function (blob) {
            if (blob) { var u = URL.createObjectURL(blob); objectUrls.push(u); img.src = u; }
          })
          .catch(function () { /* 缩略图加载失败不影响列表 */ });
        if (m.inUse) {
          var badge = document.createElement('span');
          badge.className = 'badge';
          badge.textContent = '✔使用中';
          item.appendChild(badge);
        }
        var del = document.createElement('button');
        del.className = 'del';
        del.textContent = '🗑';
        del.title = '删除';
        del.addEventListener('click', function () { deleteImage(m.id, m.file_name); });
        item.appendChild(img);
        var meta = document.createElement('div');
        meta.className = 'meta';
        meta.textContent = m.fileName + ' · ' + m.width + '×' + m.height + ' · ' + fmtSize(m.sizeBytes);
        item.appendChild(meta);
        item.appendChild(del);
        grid.appendChild(item);
      });
    }).catch(function () { toast('网络错误'); });
  }

  function fmtSize(n) {
    if (n >= 1048576) { return (n / 1048576).toFixed(1) + 'MB'; }
    return Math.round(n / 1024) + 'KB';
  }

  function deleteImage(id, name) {
    // 简易二次确认：confirm() 在移动浏览器可用（P0）；确认后删除
    if (!window.confirm('删除图片「' + name + '」？若它正被使用将回退为时钟模式。')) return;
    api('DELETE', '/images/' + id).then(function (r) {
      if (r.status === 200 && r.body.code === 0) { loadImages(); pollNow(); }
      else { toast(errText(r.body)); }
    }).catch(function () { toast('网络错误'); });
  }

  $('uploadBtn').addEventListener('click', function () { $('fileInput').click(); });
  $('fileInput').addEventListener('change', function () {
    var f = this.files[0];
    this.value = '';
    if (!f) return;
    if (f.size > 10 * 1024 * 1024) { toast('图片过大（上限 10MB）'); return; }
    var fd = new FormData();
    fd.append('file', f);
    toast('上传中…');
    api('POST', '/images', fd, true).then(function (r) {
      if (r.status === 200 && r.body.code === 0) {
        toast('上传成功，已设为使用中');
        loadImages();
        pollNow();
      } else if (r.body.code === 41301) {
        toast(r.body.msg || '图片过大/总量超限');
      } else if (r.body.code === 41501) {
        toast('仅支持 jpg/png/webp');
      } else {
        toast(errText(r.body));
      }
    }).catch(function () { toast('上传失败，请检查网络'); });
  });

  // ==================== 启动 ====================

  setOffline(false);
  pollNow();
})();
