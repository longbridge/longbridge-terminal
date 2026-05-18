(function () {
  var raw = __JSON__;

  var isHistory = Array.isArray(raw.historical) && raw.historical.length > 0;

  if (isHistory) {
    var periods = raw.historical.map(function (p) { return p.date || ''; });
    var segNames = [];
    raw.historical.forEach(function (p) {
      (p.business || []).forEach(function (b) {
        if (segNames.indexOf(b.name) === -1) segNames.push(b.name);
      });
    });
    if (!segNames.length) return;

    var series = segNames.map(function (name) {
      return {
        name: name, type: 'bar', stack: 's', barMaxWidth: 20,
        data: raw.historical.map(function (p) {
          var found = (p.business || []).filter(function (b) { return b.name === name; })[0];
          return found ? (parseFloat(found.percent) || 0) : 0;
        }),
      };
    });

    chart.setOption({
      animationDuration: 600, animationEasing: 'cubicOut',
      tooltip: {
        trigger: 'axis', axisPointer: { type: 'shadow' }, backgroundColor: '#0e0e0e',
        borderColor: '#282828', textStyle: { color: '#feffff', fontSize: 11 },
      },
      legend: { top: 4, right: 8, textStyle: { color: '#677179', fontSize: 10 } },
      grid: { left: 56, right: 16, top: 36, bottom: 64 },
      xAxis: {
        type: 'category', data: periods,
        axisLabel: { color: '#677179', fontSize: 10 },
        axisLine: { lineStyle: { color: '#282828' } },
      },
      yAxis: {
        splitLine: { lineStyle: { color: '#282828' } },
        axisLabel: { color: '#677179', fontSize: 10, formatter: function (v) { return v + '%'; } },
      },
      dataZoom: [{ type: 'inside' }, { bottom: 4, height: 20, borderColor: '#282828' }],
      series: series,
    });

    var cols = ['period'].concat(segNames);
    var rows = raw.historical.map(function (p) {
      var row = [p.date || ''];
      segNames.forEach(function (name) {
        var found = (p.business || []).filter(function (b) { return b.name === name; })[0];
        row.push(found ? (found.percent || '-') : '-');
      });
      return row;
    });
    buildTable(cols, rows);

  } else {
    var items = raw.business || [];
    if (!items.length) return;

    chart.setOption({
      animationDuration: 600, animationEasing: 'cubicOut',
      tooltip: {
        trigger: 'item',
        formatter: function (p) { return p.name + ': ' + p.value + '%'; },
        backgroundColor: '#0e0e0e', borderColor: '#282828', textStyle: { color: '#feffff', fontSize: 11 },
      },
      legend: { top: 4, right: 8, textStyle: { color: '#677179', fontSize: 10 }, orient: 'vertical' },
      series: [{
        type: 'pie', radius: ['35%', '65%'], center: ['45%', '55%'],
        data: items.map(function (b) { return { name: b.name, value: parseFloat(b.percent) || 0 }; }),
        label: { color: '#677179', fontSize: 10 },
        itemStyle: { borderColor: '#0e0e0e', borderWidth: 1 },
      }],
    });

    function fmtVal(v) {
      if (!v || v === '-') return '-';
      var n = parseFloat(v);
      if (isNaN(n)) return '-';
      var abs = Math.abs(n);
      if (abs >= 1e12) return (n / 1e12).toFixed(2) + 'T';
      if (abs >= 1e9)  return (n / 1e9).toFixed(2) + 'B';
      if (abs >= 1e6)  return (n / 1e6).toFixed(2) + 'M';
      if (abs >= 1e3)  return (n / 1e3).toFixed(2) + 'K';
      return n.toFixed(2);
    }
    function fmtYoy(v) {
      if (!v || v === '-') return '-';
      var n = parseFloat(v);
      return isNaN(n) ? '-' : n.toFixed(2) + '%';
    }

    var cols = ['segment', 'percent', 'value', 'yoy'];
    var rows = items.map(function (b) {
      return [b.name || '', b.percent ? b.percent + '%' : '-', fmtVal(b.value), fmtYoy(b.yoy)];
    });
    buildTable(cols, rows);
  }
})();
