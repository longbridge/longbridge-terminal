(function () {
  var raw = __JSON__;
  var indicators = raw.indicators || [];
  if (!indicators.length) return;

  // Chart: horizontal bar of YoY % change for indicators that have it
  var yoyInds = indicators.filter(function (ind) {
    return ind.yoy && ind.yoy !== '' && !isNaN(parseFloat(ind.yoy));
  });

  if (!yoyInds.length) {
    document.getElementById('chart').style.display = 'none';
  } else {
    var names = yoyInds.map(function (ind) { return ind.indicator_name; });
    var yoys = yoyInds.map(function (ind) { return parseFloat(ind.yoy) * 100; });

    var h = Math.max(200, names.length * 28 + 80);
    document.getElementById('chart').style.height = h + 'px';
    chart.resize();

    chart.setOption({
      animationDuration: 600, animationEasing: 'cubicOut',
      tooltip: {
        trigger: 'axis', axisPointer: { type: 'shadow' }, backgroundColor: '#0e0e0e',
        borderColor: '#282828', textStyle: { color: '#feffff', fontSize: 11 },
        formatter: function (params) {
          return params[0].axisValue + ' YoY: ' + params[0].value.toFixed(2) + '%';
        },
      },
      grid: { left: 120, right: 16, top: 36, bottom: 32 },
      xAxis: {
        type: 'value', splitLine: { lineStyle: { color: '#282828' } },
        axisLabel: { color: '#677179', fontSize: 10, formatter: function (v) { return v.toFixed(1) + '%'; } },
      },
      yAxis: {
        type: 'category', data: names,
        axisLabel: { color: '#677179', fontSize: 10 },
        axisLine: { lineStyle: { color: '#282828' } },
      },
      series: [{
        name: 'YoY Change', type: 'bar', barMaxWidth: 16,
        data: yoys,
        itemStyle: {
          color: function (params) { return params.value >= 0 ? '#44AA66' : '#D94400'; },
        },
      }],
    });
  }

  function fmtIndVal(v) {
    if (!v || v === '-') return '-';
    if (/[%]/.test(v)) return v;
    var s = v.replace(/^\$/, '');
    var n = parseFloat(s);
    if (isNaN(n)) return v;
    var pfx = v.startsWith('$') ? '$' : '';
    var abs = Math.abs(n);
    if (abs >= 1e12) return pfx + (n / 1e12).toFixed(2) + 'T';
    if (abs >= 1e9)  return pfx + (n / 1e9).toFixed(2) + 'B';
    if (abs >= 1e6)  return pfx + (n / 1e6).toFixed(2) + 'M';
    if (abs >= 1e3)  return pfx + (n / 1e3).toFixed(2) + 'K';
    return v;
  }

  var cols = ['indicator', 'value', 'yoy'];
  var rows = indicators.map(function (ind) {
    var yoy = ind.yoy && ind.yoy !== '' ? (parseFloat(ind.yoy) * 100).toFixed(2) + '%' : '-';
    return [ind.indicator_name || '', fmtIndVal(ind.indicator_value), yoy];
  });
  buildTable(cols, rows);
})();
