(function () {
  var raw = __JSON__;
  var eHist = raw.evaluate_history || [];
  if (!eHist.length) return;

  function ts2date(ts) {
    var d = new Date(parseInt(ts, 10) * 1000);
    return d.toISOString().slice(0, 10);
  }

  var xData = eHist.map(function (e) { return ts2date(e.start_date); });

  chart.setOption({
    animationDuration: 600, animationEasing: 'cubicOut',
    tooltip: { trigger: 'axis', axisPointer: { type: 'shadow' }, backgroundColor: '#0e0e0e', borderColor: '#282828', textStyle: { color: '#feffff', fontSize: 11 } },
    legend: { top: 4, right: 8, textStyle: { color: '#677179', fontSize: 10 } },
    grid: { left: 56, right: 16, top: 36, bottom: 48 },
    xAxis: { type: 'category', data: xData, axisLabel: { color: '#677179', fontSize: 10 }, axisLine: { lineStyle: { color: '#282828' } } },
    yAxis: { scale: true, splitLine: { lineStyle: { color: '#282828' } }, axisLabel: { color: '#677179', fontSize: 10 } },
    dataZoom: [{ type: 'inside' }, { bottom: 4, height: 20, borderColor: '#282828' }],
    series: [
      { name: 'Buy/Outperform', type: 'bar', stack: 'rating', barMaxWidth: 20,
        data: eHist.map(function (e) { return (parseInt(e.buy,10)||0) + (parseInt(e.over,10)||0); }),
        itemStyle: { color: '#44AA66' } },
      { name: 'Hold', type: 'bar', stack: 'rating', barMaxWidth: 20,
        data: eHist.map(function (e) { return parseInt(e.hold,10) || 0; }),
        itemStyle: { color: '#D4BC00' } },
      { name: 'Sell/Underperform', type: 'bar', stack: 'rating', barMaxWidth: 20,
        data: eHist.map(function (e) { return (parseInt(e.sell,10)||0) + (parseInt(e.under,10)||0); }),
        itemStyle: { color: '#D94400' } },
    ],
  });

  var cols = ['date', 'buy', 'outperform', 'hold', 'underperform', 'sell', 'total'];
  var rows = eHist.map(function (e) {
    return [ts2date(e.start_date), e.buy, e.over, e.hold, e.under, e.sell, e.total];
  });
  buildTable(cols, rows);
})();
