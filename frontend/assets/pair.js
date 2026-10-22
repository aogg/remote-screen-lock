/**
 * 配对页逻辑（F-P0-04 / US-01）
 * - 扫码进入：URL 携带 #t=<token> → 静默调 POST /api/v1/pair/verify → 成功直接进控制台
 * - 手动输码：POST /api/v1/pair { code } → 成功保存 token（localStorage rsl.tokens 支持多台电脑）
 * - 失败反馈：40103 剩余次数；42901 锁定倒计时（5 分钟内禁止重试）
 */
(function () {
  'use strict';
  var $ = function (id) { return document.getElementById(id); };

  /** token 仓库：{ baseUrl: token }（一台手机可配对多台电脑） */
  function loadStore() {
    try { return JSON.parse(localStorage.getItem('rsl.tokens') || '{}'); }
    catch (e) { return {}; }
  }
  function saveToken(base, token) {
    var s = loadStore();
    s[base] = token;
    localStorage.setItem('rsl.tokens', JSON.stringify(s));
  }
  function currentToken() {
    var s = loadStore();
    return s[location.origin] || '';
  }

  function toast(msg) {
    var t = $('toast');
    t.textContent = msg;
    t.classList.add('show');
    setTimeout(function () { t.classList.remove('show'); }, 2600);
  }

  // ==================== 扫码路径：#t=<token> 静默校验 ====================

  function tryTokenFromHash() {
    var m = /[#&]t=([A-Za-z0-9]+)/.exec(location.hash);
    if (!m) return false;
    var token = m[1];
    // 清掉 hash 避免 token 残留在地址栏/历史
    history.replaceState(null, '', location.pathname);
    fetch('/api/v1/pair/verify', { method: 'POST', headers: { 'Authorization': 'Bearer ' + token } })
      .then(function (res) {
        if (!res.ok) throw new Error('verify ' + res.status);
        saveToken(location.origin, token);
        location.replace('/');
      })
      .catch(function () {
        $('err').textContent = '二维码已失效，请手动输入配对码（或到电脑端重新生成二维码）';
      });
    return true;
  }

  // 扫码路径优先；无 token 且未扫码时若本机已配对过，直接进入控制台
  var fromHash = tryTokenFromHash();
  if (!fromHash && currentToken()) {
    location.replace('/');
    return;
  }

  // ==================== 手动输码 ====================

  $('base').value = location.origin;
  $('go').addEventListener('click', submit);
  $('code').addEventListener('keydown', function (e) {
    if (e.key === 'Enter') submit();
  });

  var lockTimer = null;

  function submit() {
    var base = $('base').value.trim().replace(/\/+$/, '');
    var code = $('code').value.trim();
    var err = $('err');
    err.textContent = '';
    $('code').classList.remove('err');

    // 前置校验
    if (!/^https?:\/\/.+:?\d*$/.test(base)) {
      err.textContent = '电脑地址格式不正确，应形如 http://192.168.1.244:8326';
      return;
    }
    var norm = code.replace(/[^0-9a-zA-Z]/g, '').toUpperCase();
    if (norm.length !== 8) {
      err.textContent = '请输入 8 位配对码（在电脑 设置→配对 查看）';
      $('code').classList.add('err');
      return;
    }

    var btn = $('go');
    btn.disabled = true;
    btn.textContent = '连接中…';
    // 注意：手动输码允许跨 baseUrl（手机直接访问输入的电脑地址）
    fetch(base + '/api/v1/pair', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ code: code, device: navigator.userAgent.slice(0, 60) })
    }).then(function (res) {
      return res.json().then(function (j) { return { status: res.status, body: j }; });
    }).then(function (r) {
      if (r.status === 200 && r.body.code === 0 && r.body.data && r.body.data.token) {
        saveToken(base, r.body.data.token);
        toast('配对成功');
        setTimeout(function () {
          if (base === location.origin) location.replace('/');
          else location.href = base + '/'; // 访问的是另一台电脑地址
        }, 400);
        return;
      }
      // 失败分支：按错误码给出可读反馈
      btn.disabled = false;
      btn.textContent = '连 接';
      var biz = r.body && r.body.code;
      if (biz === 42901) {
        var secs = (r.body.data && r.body.data.retryAfterSec) || 300;
        lockInput(secs);
      } else if (biz === 40103) {
        err.textContent = r.body.msg || '配对码不正确';
        $('code').classList.add('err');
      } else {
        err.textContent = r.body.msg || ('连接失败（' + r.status + '），请检查地址与网络');
      }
    }).catch(function () {
      btn.disabled = false;
      btn.textContent = '连 接';
      err.textContent = '无法连接被控端，请确认手机与电脑在同一 Wi-Fi';
    });
  }

  /** 5 次输错锁定：按钮禁用 + mm:ss 倒计时（US-01） */
  function lockInput(secs) {
    var btn = $('go'), err = $('err');
    btn.disabled = true;
    var left = secs;
    var fmt = function (n) {
      var m = Math.floor(n / 60), s = n % 60;
      return m + ':' + ('0' + s).slice(-2);
    };
    err.textContent = '尝试次数过多，' + fmt(left) + ' 内禁止重试';
    clearInterval(lockTimer);
    lockTimer = setInterval(function () {
      left -= 1;
      if (left <= 0) {
        clearInterval(lockTimer);
        btn.disabled = false;
        err.textContent = '';
        return;
      }
      err.textContent = '尝试次数过多，' + fmt(left) + ' 内禁止重试';
    }, 1000);
  }
})();
