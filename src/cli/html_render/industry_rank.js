(function () {
  var raw = __JSON__;
  var lists = (raw.items && raw.items[0] && raw.items[0].lists) || [];
  if (!lists.length) return;

  document.getElementById('chart').style.display = 'none';

  var sorted = lists.slice().sort(function (a, b) {
    return (parseFloat(b.chg) || 0) - (parseFloat(a.chg) || 0);
  });

  function fmtPct(v) {
    if (!v || v === '-') return '-';
    var n = parseFloat(v);
    if (isNaN(n)) return '-';
    return (n * 100).toFixed(2) + '%';
  }

  var cols = ['industry', 'counter_id', 'chg%', 'leading', 'leading_chg%'];
  var rows = sorted.map(function (it) {
    var leading = it.leading_name
      ? it.leading_name + ' (' + (it.leading_ticker || '') + ')'
      : '-';
    return [it.name || '', it.counter_id || '', fmtPct(it.chg), leading, fmtPct(it.leading_chg)];
  });
  buildTable(cols, rows);
})();
