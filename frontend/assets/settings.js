/**
 * 设置窗逻辑（F-P0-15）：tab 懒加载（切换时才拉数据）、配对码倒计时、
 * 二次确认的重置 token、全部设置持久化、日志查看。
 * 与后端通信：window.__TAURI__.core.invoke（无 HTTP，桌面本机信任通道）。
 */
(function () {
  'use strict';
  var $ = function (id) { return document.getElementById(id); };
  var invoke = window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke;
  if (!invoke) {
    document.body.innerHTML = '<div style="padding:40px;color:#EF4444">初始化失败：Tauri IPC 不可用</div>';
    return;
  }

  var settings = null;           // 后端返回的完整设置快照
  var dirty = {};                // 待保存修改
  var loadedTabs = {};           // tab 懒加载标记（首次切入才拉数据）
  var expireTimer = null;

  // ==================== tab 切换（懒加载） ====================

  document.querySelectorAll('.side button[data-tab]').forEach(function (btn) {
    btn.addEventListener('click', function () {
      var tab = btn.dataset.tab;
      document.querySelectorAll('.side button').forEach(function (b) { b.classList.toggle('active', b === btn); });
      document.querySelectorAll('.panel').forEach(function (p) { p.classList.toggle('active', p.id === 'panel-' + tab); });
      if (!loadedTabs[tab]) {
        loadedTabs[tab] = true;
        if (tab === 'logs') { loadLogs(); }
        if (tab === 'pair') { loadSettings(); }
      }
    });
  });

  // ==================== 设置加载 ====================

  function loadSettings() {
    invoke('get_settings').then(function (s) {
      settings = s;
      renderPairing(s.pairing);
      // token 脱敏展示
      $('tokenPreview').textContent = s.pairing.tokenPreview;
      // 常规
      $('inPort').value = s.config.port;
      if (s.portChanged) {
        $('portWarn').classList.add('show');
        $('actualPort').textContent = s.actualPort;
      } else {
        $('portWarn').classList.remove('show');
      }
      // 锁屏
      markRadio('rgMode', 'mode', s.config.defaultMode);
      markRadio('rgHour', 'hour', s.config.hour12 ? '12' : '24');
      $('inMask').value = s.config.imageMaskPercent;
      $('maskVal').textContent = s.config.imageMaskPercent + '%';
      // 安全
      $('swSysLock').classList.toggle('on', s.config.systemLockEnabled);
      $('selHotkey').value = s.config.unlockHotkey;
      $('keyHint').textContent = s.config.localKeyIsUserSet
        ? '密钥仅以 argon2id 哈希存储，配置文件不含明文'
        : '首次使用请设置密钥（不设置也可用快捷键解锁）';
      $('verInfo').textContent = s.deviceName + ' · v' + s.version + ' · ' + s.os;
      dirty = {};
    }).catch(function (e) {
      alert('读取设置失败：' + e);
    });
  }

  /** 配对信息渲染：二维码 SVG + 配对码 + 倒计时 */
  function renderPairing(p) {
    if (!p) return;
    $('qrBox').innerHTML = p.qrSvg || '';
    $('pairCode').textContent = p.codeFormatted || p.code;
    $('pairUrls').textContent = (p.urls || []).join(' ｜ ');
    // 倒计时（10 分钟有效期）
    clearInterval(expireTimer);
    var left = Math.max(0, p.expiresAtUnix - Math.floor(Date.now() / 1000));
    var fmt = function (n) {
      var m = Math.floor(n / 60), s = n % 60;
      return m + ':' + ('0' + s).slice(-2);
    };
    var tick = function () {
      $('pairExpire').textContent = left > 0 ? ('有效期剩 ' + fmt(left)) : '已过期，请重新生成';
      if (left <= 0) { clearInterval(expireTimer); }
      left -= 1;
    };
    tick();
    expireTimer = setInterval(tick, 1000);
  }

  // ==================== 配对操作 ====================

  $('btnRegenCode').addEventListener('click', function () {
    invoke('regenerate_pairing').then(renderPairing);
  });

  // 重置 token：两步确认（红字说明所有已配对手机将失效）
  var resetArmed = false, resetTimer = null;
  $('btnResetToken').addEventListener('click', function () {
    if (!resetArmed) {
      resetArmed = true;
      this.textContent = '确认重置？（所有已配对手机将失效）';
      var btn = this;
      clearTimeout(resetTimer);
      resetTimer = setTimeout(function () {
        resetArmed = false;
        btn.textContent = '重置 token';
      }, 4000);
      return;
    }
    clearTimeout(resetTimer);
    resetArmed = false;
    this.textContent = '重置 token';
    invoke('reset_token').then(function (r) {
      $('tokenPreview').textContent = r.tokenPreview;
      return invoke('get_settings');
    }).then(function (s) {
      settings = s;
      renderPairing(s.pairing);
      $('tokenPreview').textContent = s.pairing.tokenPreview;
    });
  });

  // ==================== 表单交互 ====================

  function markRadio(groupId, attr, val) {
    $(groupId).querySelectorAll('button').forEach(function (b) {
      b.classList.toggle('active', b.dataset[attr] === String(val));
    });
  }

  $('rgMode').addEventListener('click', function (e) {
    var b = e.target.closest('button');
    if (!b) return;
    dirty.defaultMode = b.dataset.mode;
    markRadio('rgMode', 'mode', b.dataset.mode);
  });
  $('rgHour').addEventListener('click', function (e) {
    var b = e.target.closest('button');
    if (!b) return;
    dirty.hour12 = b.dataset.hour === '12';
    markRadio('rgHour', 'hour', b.dataset.hour);
  });
  $('inMask').addEventListener('input', function () {
    $('maskVal').textContent = this.value + '%';
    dirty.imageMaskPercent = Number(this.value);
  });
  $('inPort').addEventListener('change', function () {
    dirty.port = Number(this.value);
  });
  $('selHotkey').addEventListener('change', function () {
    dirty.unlockHotkey = this.value;
  });
  $('swSysLock').addEventListener('click', function () {
    var on = !this.classList.contains('on');
    this.classList.toggle('on', on);
    dirty.systemLockEnabled = on;
  });

  // 保存全部（持久化；端口重启生效）
  $('btnSaveAll').addEventListener('click', function () {
    var payload = Object.keys(dirty).length ? dirty : {};
    if (payload.defaultMode === 'image' && settings && settings.config.imageCount === 0) {
      alert('尚未上传屏保图片，无法设为图片模式。请先在手机控制台上传。');
      return;
    }
    invoke('save_settings', { payload: payload }).then(function () {
      $('savedTip').textContent = '已保存 ✓' + (payload.port !== undefined ? '（端口修改重启应用后生效）' : '');
      setTimeout(function () { $('savedTip').textContent = ''; }, 4000);
      dirty = {};
      loadSettings();
    }).catch(function (e) {
      alert('保存失败：' + e);
    });
  });

  // 修改密钥
  $('btnSaveKey').addEventListener('click', function () {
    var oldKey = $('inOldKey').value.trim() || null;
    var newKey = $('inNewKey').value.trim();
    invoke('change_local_key', { oldKey: oldKey, newKey: newKey }).then(function () {
      alert('密钥已设置（仅存哈希，请牢记）');
      $('inOldKey').value = '';
      $('inNewKey').value = '';
      loadSettings();
    }).catch(function (e) {
      alert('设置失败：' + e);
    });
  });

  // ==================== 日志 ====================

  function loadLogs() {
    invoke('get_logs').then(function (r) {
      var tbody = $('logBody');
      tbody.innerHTML = '';
      (r.logs || []).slice().reverse().forEach(function (l) {
        var tr = document.createElement('tr');
        var t = new Date(l.ts * 1000);
        var cells = [
          t.toLocaleString('zh-CN', { hour12: false }),
          l.source, l.action,
        ];
        cells.forEach(function (c) {
          var td = document.createElement('td');
          td.textContent = c;
          tr.appendChild(td);
        });
        var td = document.createElement('td');
        td.textContent = l.result;
        td.className = l.result.indexOf('ok') === 0 ? 'result-ok' : 'result-err';
        tr.appendChild(td);
        tbody.appendChild(tr);
      });
      if (!(r.logs || []).length) {
        tbody.innerHTML = '<tr><td colspan="4" style="color:#6B7280">暂无日志</td></tr>';
      }
    });
  }
  $('btnReloadLogs').addEventListener('click', loadLogs);

  // ==================== 后端事件：配置被远程修改时刷新 ====================

  var ev = window.__TAURI__ && window.__TAURI__.event;
  if (ev && ev.listen) {
    ev.listen('rsl://config-changed', function () {
      // 避免与本地未保存修改冲突：仅无脏数据时自动刷新
      if (Object.keys(dirty).length === 0) { loadSettings(); }
    });
  }

  // ==================== 启动：加载首个 tab ====================

  loadedTabs.pair = true;
  loadSettings();
})();
