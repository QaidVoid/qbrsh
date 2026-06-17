// Hint-mode engine injected into every page as window.__qbrshHints.
//
// The Rust core drives it through small evaluate_javascript calls:
//   show(chars)        -> render labels, returns space-joined label list
//   filter(prefix)     -> hide labels not matching the typed prefix
//   followClick(label) -> focus+click the element (current tab)
//   getHref(label)     -> return the element's href (open in new tab)
//   clear()            -> remove all labels
window.__qbrshHints = (function () {
  var SELECTOR = [
    'a[href]', 'button', 'input:not([type=hidden])', 'textarea', 'select',
    'summary', '[onclick]', '[role=button]', '[role=link]', '[role=tab]',
    '[contenteditable]:not([contenteditable=false])', '[tabindex]:not([tabindex="-1"])'
  ].join(', ');

  var labels = {};
  var container = null;

  function visible(el) {
    var r = el.getBoundingClientRect();
    if (r.width <= 0 || r.height <= 0) return false;
    if (r.bottom < 0 || r.right < 0 || r.top > innerHeight || r.left > innerWidth) return false;
    var s = getComputedStyle(el);
    return s.visibility !== 'hidden' && s.display !== 'none' && s.opacity !== '0';
  }

  function genLabels(n, chars) {
    var alpha = chars.split('');
    var len = 1, cap = alpha.length;
    while (cap < n) { len++; cap *= alpha.length; }
    var out = [];
    (function build(prefix, depth) {
      if (out.length >= n) return;
      if (depth === len) { out.push(prefix); return; }
      for (var i = 0; i < alpha.length && out.length < n; i++) build(prefix + alpha[i], depth + 1);
    })('', 0);
    return out;
  }

  function clear() {
    if (container) { container.remove(); container = null; }
    labels = {};
  }

  function show(chars) {
    clear();
    var els = Array.prototype.slice.call(document.querySelectorAll(SELECTOR)).filter(visible);
    var lbls = genLabels(els.length, chars);
    container = document.createElement('div');
    container.style.cssText = 'position:fixed;top:0;left:0;width:0;height:0;z-index:2147483647';
    for (var i = 0; i < els.length; i++) {
      labels[lbls[i]] = els[i];
      var r = els[i].getBoundingClientRect();
      var tag = document.createElement('span');
      tag.dataset.label = lbls[i];
      tag.textContent = lbls[i].toUpperCase();
      tag.style.cssText =
        'position:fixed;left:' + Math.max(0, r.left) + 'px;top:' + Math.max(0, r.top) + 'px;' +
        'background:#ffd76e;color:#000;font:bold 11px monospace;padding:1px 3px;' +
        'border:1px solid #a07000;border-radius:3px;box-shadow:0 1px 3px rgba(0,0,0,.4)';
      container.appendChild(tag);
    }
    document.documentElement.appendChild(container);
    return Object.keys(labels).join(' ');
  }

  function filter(prefix) {
    if (!container) return '';
    var tags = container.children;
    for (var i = 0; i < tags.length; i++) {
      tags[i].style.display = tags[i].dataset.label.indexOf(prefix) === 0 ? '' : 'none';
    }
    return '';
  }

  function followClick(label) {
    var el = labels[label];
    clear();
    if (el) { if (el.focus) el.focus(); if (el.click) el.click(); }
    return '';
  }

  function getHref(label) {
    var el = labels[label];
    clear();
    return el && el.href ? el.href : '';
  }

  return { show: show, filter: filter, followClick: followClick, getHref: getHref, clear: clear };
})();
