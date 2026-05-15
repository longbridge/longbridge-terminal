(function () {
  var raw = __JSON__;
  var METRICS = [
    { key: 'pe', label: 'PE' },
    { key: 'pb', label: 'PB' },
    { key: 'ps', label: 'PS' },
    { key: 'dvd', label: 'Div' },
  ];
  var active = METRICS.filter(function (m) { return (raw[m.key] || []).length > 0; });
  if (!active.length) return;

  function ts2date(ts) {
    var d = new Date(parseInt(ts, 10) * 1000);
    return d.toISOString().slice(0, 10);
  }

  var xData = (raw[active[0].key] || []).map(function (e) { return ts2date(e.timestamp); });

  var series = active.map(function (m) {
    var data = (raw[m.key] || []).map(function (e) {
      var rank = parseInt(e.rank, 10);
      var total = parseInt(e.total, 10);
      return total ? parseFloat(((total - rank + 1) / total * 100).toFixed(1)) : null;
    });
    return { name: m.label, type: 'line', data: data, showSymbol: false, lineStyle: { width: 2 } };
  });

  chart.setOption({
    animationDuration: 600, animationEasing: 'cubicOut',
    tooltip: {
      trigger: 'axis', backgroundColor: '#0e0e0e', borderColor: '#282828',
      textStyle: { color: '#feffff', fontSize: 11 },
      formatter: function (params) {
        var s = params[0].axisValue + '<br/>';
        params.forEach(function (p) { if (p.value != null) s += p.marker + p.seriesName + ': ' + p.value + '%<br/>'; });
        return s;
      }
    },
    legend: { top: 4, right: 8, textStyle: { color: '#677179', fontSize: 10 } },
    grid: { left: 56, right: 16, top: 36, bottom: 64 },
    xAxis: {
      type: 'category', data: xData,
      axisLabel: { color: '#677179', fontSize: 10 },
      axisLine: { lineStyle: { color: '#282828' } }
    },
    yAxis: {
      min: 0, max: 100,
      name: 'Percentile', nameTextStyle: { color: '#677179', fontSize: 9 },
      splitLine: { lineStyle: { color: '#282828' } },
      axisLabel: { color: '#677179', fontSize: 10, formatter: function (v) { return v + '%'; } }
    },
    dataZoom: [{ type: 'inside' }, { bottom: 4, height: 20, borderColor: '#282828' }],
    series: series,
  });

  var cols = ['date'].concat(active.map(function (m) { return m.label + ' (rank/total)'; }));
  var rows = xData.map(function (date, i) {
    var row = [date];
    active.forEach(function (m) {
      var e = (raw[m.key] || [])[i];
      row.push(e ? e.rank + '/' + e.total : '-');
    });
    return row;
  });
  buildTable(cols, rows);
})();
