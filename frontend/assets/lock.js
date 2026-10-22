/**
 * 锁定窗口逻辑（零构建，经 window.__TAURI__ 与 Rust 通信）
 * - 三模式渲染：clock / image / black（F-P0-07）
 * - 时钟每秒读系统时钟，仅整分变更才重绘（F-P0-08：不跳变、省电）
 * - 防误操作：移动鼠标/单击/普通按键不退出；Esc 唤出密钥浮层；快捷键直接解锁（F-P0-09）
 */
(function () {
  'use strict';
  // Tauri 全局 API（app.withGlobalTauri = true）
  var T = window.__TAURI__ || {};
  var invoke = T.core && T.core.invoke;
  var convertFileSrc = T.core && T.core.convertFileSrc;

  var state = {
    mode: 'clock',          // 当前模式
    hour12: false,          // 时间制式
    maskPercent: 30,        // 图片遮罩
    hotkey: 'ctrl+alt+q',   // 快捷键描述
    lastMinuteText: '',     // 上次渲染的分钟文本（避免重绘）
    hintTimer: null,
    frozenTimer: null,
    unlocked: false,        // 解锁中标记（防止重复 invoke）
  };

  var $ = function (id) { return document.getElementById(id); };

  // ==================== 模式渲染 ====================

  /** 应用模式到 DOM（300ms 淡入淡出由 CSS transition 承担） */
  function applyMode() {
    var body = document.body;
    body.className = 'lockpage mode-' + state.mode + (hintVisible ? ' showcursor' : '');
    var isImage = state.mode === 'image';
    $('bg').style.display = isImage ? 'block' : 'none';
    $('mask').style.display = isImage ? 'block' : 'none';
    $('mask').style.opacity = isImage ? String(Math.min(state.maskPercent, 60) / 100) : '0';
    renderClock(true);
    $('hotkeyLabel').textContent = hotkeyDisplay(state.hotkey);
  }

  /** 图片模式：取当前使用中的图片绝对路径并经 asset 协议转换 */
  function loadImage() {
    if (!invoke) return;
    invoke('get_lock_image').then(function (res) {
      if (res && res.path) {
        var url = convertFileSrc ? convertFileSrc(res.path) : ('asset://' + res.path);
        $('bg').style.backgroundImage = 'url("' + url + '")';
      } else {
        // 无图兜底：退回时钟观感
        $('bg').style.display = 'none';
        $('mask').style.display = 'none';
      }
    }).catch(function (e) {
      // 失败兜底：保持深色背景 + 时钟
      console.error('load image failed', e);
      $('bg').style.display = 'none';
      $('mask').style.display = 'none';
    });
  }

  // ==================== 时钟（F-P0-08） ====================

  /** 渲染时间/日期；force=true 或跨分钟时更新文本 */
  function renderClock(force) {
    var now = new Date();
    var hh = now.getHours();
    var suffix = '';
    if (state.hour12) {
      suffix = hh >= 12 ? ' PM' : ' AM';
      hh = hh % 12; if (hh === 0) { hh = 12; }
    }
    var mm = ('0' + now.getMinutes()).slice(-2);
    var text = ('0' + hh).slice(-2) + ':' + mm + suffix;
    if (force || text !== state.lastMinuteText) {
      state.lastMinuteText = text;
      $('time').textContent = text;
      // 日期星期跟随系统语言区域（如 10月22日 星期四 / Oct 22, Thu）
      try {
        var locale = navigator.language || 'zh-CN';
        var dateStr = new Intl.DateTimeFormat(locale, { month: 'short', day: 'numeric' }).format(now);
        var weekStr = new Intl.DateTimeFormat(locale, { weekday: 'short' }).format(now);
        $('date').textContent = dateStr + ' ' + weekStr;
      } catch (e) {
        $('date').textContent = now.toLocaleDateString();
      }
    }
  }

  // ==================== 提示条 [C] ====================

  var hintVisible = false;
  function showHint() {
    hintVisible = true;
    document.body.classList.add('showcursor');
    $('hint').classList.add('show');
    clearTimeout(state.hintTimer);
    state.hintTimer = setTimeout(hideHint, 3000); // 3 秒无操作隐藏
  }
  function hideHint() {
    hintVisible = false;
    document.body.classList.remove('showcursor');
    $('hint').classList.remove('show');
  }

  // ==================== 密钥浮层 [D] ====================

  function openKeyOverlay() {
    $('keyOverlay').classList.add('show');
    setTimeout(function () { $('keyInput').focus(); }, 60);
  }
  function closeKeyOverlay() {
    $('keyOverlay').classList.remove('show');
    $('keyInput').value = '';
    $('keyErr').textContent = '';
    clearTimeout(state.frozenTimer);
  }

  /** 提交密钥：ok→解锁；错误→抖动；5 次错→冻结倒计时（F-P0-09） */
  function submitKey() {
    if (!invoke || state.unlocked) return;
    var key = $('keyInput').value.trim();
    if (!key) { $('keyInput').focus(); return; }
    invoke('try_local_unlock', { key: key }).then(function (r) {
      if (r && r.ok) {
        state.unlocked = true;
        closeKeyOverlay();
        return; // 窗口即将被后端销毁
      }
      if (r && r.frozenRemainingSec > 0) {
        freezeCountdown(r.frozenRemainingSec);
        return;
      }
      var left = r ? r.remainingAttempts : 0;
      $('keyErr').textContent = left > 0
        ? ('密钥不正确（剩余 ' + left + ' 次机会）')
        : '密钥不正确';
      var inp = $('keyInput');
      inp.classList.remove('err');
      void inp.offsetWidth; // 重置动画
      inp.classList.add('err');
      inp.value = '';
      inp.focus();
    }).catch(function (e) {
      $('keyErr').textContent = '解锁失败：' + e;
    });
  }

  /** 冻结倒计时：按钮禁用 + 剩余秒数提示（30 秒） */
  function freezeCountdown(sec) {
    var btn = $('keyGo'), cancel = $('keyCancel'), inp = $('keyInput');
    btn.disabled = true; cancel.disabled = true; inp.disabled = true;
    var left = sec;
    $('keyErr').textContent = '尝试次数过多，已冻结 ' + left + ' 秒';
    clearTimeout(state.frozenTimer);
    (function tick() {
      left -= 1;
      if (left <= 0) {
        btn.disabled = false; cancel.disabled = false; inp.disabled = false;
        $('keyErr').textContent = '';
        inp.value = '';
        inp.focus();
        return;
      }
      $('keyErr').textContent = '尝试次数过多，已冻结 ' + left + ' 秒';
      state.frozenTimer = setTimeout(tick, 1000);
    })();
  }

  // ==================== 快捷键 ====================

  /** "ctrl+alt+q" → 显示 "Ctrl+Alt+Q" */
  function hotkeyDisplay(h) {
    return (h || 'ctrl+alt+q').split('+').map(function (p) {
      return { ctrl: 'Ctrl', alt: 'Alt', shift: 'Shift' }[p] || p.toUpperCase();
    }).join('+');
  }

  /** 解析热键描述为 keydown 匹配器 */
  function hotkeyMatcher(h) {
    var parts = (h || 'ctrl+alt+q').split('+');
    var key = parts[parts.length - 1];
    var needCtrl = parts.indexOf('ctrl') >= 0;
    var needAlt = parts.indexOf('alt') >= 0;
    var needShift = parts.indexOf('shift') >= 0;
    return function (e) {
      var k = (e.key || '').toLowerCase();
      return k === key
        && e.ctrlKey === needCtrl && e.altKey === needAlt && e.shiftKey === needShift;
    };
  }

  // ==================== 事件绑定 ====================

  function bindEvents() {
    // 顶部热区：浮现提示条（clock/image 模式由 hotzone 元素承担）
    $('hotzone').addEventListener('mouseenter', showHint);
    $('hint').addEventListener('click', openKeyOverlay);
    // 防误操作：移动/单击不退出（US-09）；黑屏模式给一次性提示（interaction 2.3）
    document.addEventListener('mousemove', function () {
      if (state.mode === 'black' && !hintVisible) showHint();
    });
    document.addEventListener('click', function () {
      if (state.mode === 'black' && !hintVisible) showHint();
    });
    document.addEventListener('keydown', function (e) {
      // 快捷键直接解锁（默认 Ctrl+Alt+Q）
      if (hotkeyMatcher(state.hotkey)(e)) {
        e.preventDefault();
        if (invoke && !state.unlocked) { state.unlocked = true; invoke('hotkey_unlock'); }
        return;
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        if ($('keyOverlay').classList.contains('show')) closeKeyOverlay();
        else openKeyOverlay();
        return;
      }
      // 普通按键不退出；黑屏模式给一次性提示（interaction 2.3）
      if (state.mode === 'black' && !hintVisible) showHint();
    });
    document.addEventListener('contextmenu', function (e) { e.preventDefault(); });

    $('keyGo').addEventListener('click', submitKey);
    $('keyCancel').addEventListener('click', closeKeyOverlay);
    $('keyInput').addEventListener('keydown', function (e) {
      if (e.key === 'Enter') { e.preventDefault(); submitKey(); }
    });

    // 后端事件：模式即时切换（无需解锁）
    if (T.event && T.event.listen) {
      T.event.listen('rsl://mode-changed', function (ev) {
        var p = (ev && ev.payload) || {};
        if (p.mode) { state.mode = p.mode; applyMode(); if (p.mode === 'image') loadImage(); }
      });
      T.event.listen('rsl://config-changed', function () { refreshSettings(); });
    }
  }

  /** 从后端拉取锁定参数（模式/制式/遮罩/快捷键） */
  function refreshSettings() {
    if (!invoke) return;
    invoke('get_lock_state').then(function (s) {
      if (!s) return;
      state.mode = s.mode || state.mode;
      state.hour12 = !!s.hour12;
      state.maskPercent = typeof s.imageMaskPercent === 'number' ? s.imageMaskPercent : 30;
      state.hotkey = s.hotkey || state.hotkey;
      applyMode();
      if (state.mode === 'image') loadImage();
    }).catch(function (e) { console.error('get_lock_state failed', e); });
  }

  // ==================== 启动 ====================

  function boot() {
    bindEvents();
    // 初始化注入参数（后端创建窗口时 eval 的 __RSL_INIT__，可能早于本脚本）
    var init = window.__RSL_INIT__;
    if (init && init.mode) {
      state.mode = init.mode;
      applyMode();
      if (state.mode === 'image' && init.imagePath) {
        var url = convertFileSrc ? convertFileSrc(init.imagePath) : ('asset://' + init.imagePath);
        $('bg').style.backgroundImage = 'url("' + url + '")';
      }
    } else {
      refreshSettings();
    }
    setInterval(function () { renderClock(false); }, 1000);
    renderClock(true);
  }

  boot();
})();
