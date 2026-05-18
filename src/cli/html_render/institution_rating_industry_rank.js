(function () {
  var raw = __JSON__;
  var items = raw.items || [];
  if (!items.length) return;

  // Sort by buy+over descending for the chart
  var sorted = items.slice().sort(function (a, b) {
    return ((+b.buy || 0) + (+b.over || 0)) - ((+a.buy || 0) + (+a.over || 0));
  });

  var names = sorted.map(function (it) { return it.name || it.symbol || ''; });
  // Expand the chart container to fit all items
  var h = Math.max(200, names.length * 22 + 56);
  document.getElementById('chart').style.height = h + 'px';
  chart.resize();

  chart.setOption({
    animationDuration: 600, animationEasing: 'cubicOut',
    tooltip: {
      trigger: 'axis', axisPointer: { type: 'shadow' }, backgroundColor: '#0e0e0e',
      borderColor: '#282828', textStyle: { color: '#feffff', fontSize: 11 }
    },
    legend: { top: 4, right: 8, textStyle: { color: '#677179', fontSize: 10 } },
    grid: { left: 130, right: 16, top: 36, bottom: 24 },
    xAxis: {
      type: 'value', splitLine: { lineStyle: { color: '#282828' } },
      axisLabel: { color: '#677179', fontSize: 10 }
    },
    yAxis: {
      type: 'category', data: names,
      axisLabel: { color: '#677179', fontSize: 10 },
      axisLine: { lineStyle: { color: '#282828' } }
    },
    series: [
      { name: 'Buy/Outperform', type: 'bar', stack: 'r', barMaxWidth: 16,
        data: sorted.map(function (it) { return (+it.buy || 0) + (+it.over || 0); }),
        itemStyle: { color: '#44AA66' } },
      { name: 'Hold', type: 'bar', stack: 'r', barMaxWidth: 16,
        data: sorted.map(function (it) { return +it.hold || 0; }),
        itemStyle: { color: '#D4BC00' } },
      { name: 'Sell/Underperform', type: 'bar', stack: 'r', barMaxWidth: 16,
        data: sorted.map(function (it) { return (+it.sell || 0) + (+it.under || 0); }),
        itemStyle: { color: '#D94400' } },
    ],
  });

  var cols = ['rank', 'symbol', 'name', 'buy', 'outperform', 'hold', 'underperform', 'sell', 'total'];
  var rows = items.map(function (it) {
    return [it.rank, it.symbol || '', it.name || '', it.buy, it.over, it.hold, it.under, it.sell, it.total];
  });
  buildTable(cols, rows);
})();
