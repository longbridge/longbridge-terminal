(function () {
  var raw = __JSON__;
  var list = raw.list || {};

  var SECTIONS = [
    { key: 'IS', label: 'Income Statement' },
    { key: 'BS', label: 'Balance Sheet' },
    { key: 'CF', label: 'Cash Flow' },
  ];

  // Key metrics to highlight in charts per section (substring match on name)
  var CHART_CFG = {
    IS: {
      bars: ['营业收入', '净利润', '营业利润'],
      lines: ['毛利率', '净利率'],
      colors: ['#4488CC', '#44AA66', '#7A7A8E', '#0088AA', '#D4BC00'],
    },
    BS: {
      bars: ['总资产', '总负债', '净债务'],
      lines: [],
      colors: ['#4488CC', '#d84a33', '#44AA66'],
    },
    CF: {
      bars: ['经营现金流', '投资现金流', '融资现金流', '自由现金流'],
      lines: [],
      colors: ['#4488CC', '#D94400', '#7A7A8E', '#44AA66'],
    },
  };

  function fmtVal(v, isPercent) {
    if (v === null || v === undefined || v === '') return '-';
    var n = parseFloat(v);
    if (isNaN(n)) return String(v);
    if (isPercent) return n.toFixed(2) + '%';
    var abs = Math.abs(n);
    if (abs >= 1e12) return (n / 1e12).toFixed(2) + 'T';
    if (abs >= 1e9)  return (n / 1e9).toFixed(2) + 'B';
    if (abs >= 1e6)  return (n / 1e6).toFixed(2) + 'M';
    if (abs >= 1e3)  return (n / 1e3).toFixed(2) + 'K';
    return n.toFixed(4).replace(/\.?0+$/, '');
  }

  function fmtAxisVal(n) {
    var abs = Math.abs(n);
    if (abs >= 1e12) return (n / 1e12).toFixed(1) + 'T';
    if (abs >= 1e9)  return (n / 1e9).toFixed(1) + 'B';
    if (abs >= 1e6)  return (n / 1e6).toFixed(1) + 'M';
    if (abs >= 1e3)  return (n / 1e3).toFixed(1) + 'K';
    return n;
  }

  // Extract periods and a metric map from sectionData
  function extractData(sectionData, maxPeriods) {
    var indicators = sectionData.indicators || [];
    var periods = [];
    var metricMap = {}; // name -> { values: [...], isPercent }
    for (var i = 0; i < indicators.length; i++) {
      var accs = indicators[i].accounts || [];
      if (accs.length && accs[0].values && accs[0].values.length) {
        periods = accs[0].values.slice(0, maxPeriods || 8).map(function (v) { return v.period || ''; });
        break;
      }
    }
    indicators.forEach(function (ind) {
      (ind.accounts || []).forEach(function (acc) {
        var valMap = {};
        (acc.values || []).forEach(function (v) { if (v.period) valMap[v.period] = v.value; });
        metricMap[acc.name] = { valMap: valMap, isPercent: !!acc.percent };
      });
    });
    return { periods: periods, metricMap: metricMap };
  }

  function findMetric(metricMap, keyword) {
    var keys = Object.keys(metricMap);
    for (var i = 0; i < keys.length; i++) {
      if (keys[i].indexOf(keyword) !== -1) return { name: keys[i], data: metricMap[keys[i]] };
    }
    return null;
  }

  function renderChart(chartId, sectionKey, sectionData) {
    var el = document.getElementById(chartId);
    if (!el) return;
    var cfg = CHART_CFG[sectionKey];
    if (!cfg) return;

    var extracted = extractData(sectionData, 8);
    var periods = extracted.periods;
    var metricMap = extracted.metricMap;
    if (!periods.length) return;

    var barSeries = [];
    var lineSeries = [];
    var hasLines = false;

    cfg.bars.forEach(function (kw) {
      var m = findMetric(metricMap, kw);
      if (!m) return;
      var vals = periods.map(function (p) {
        var v = m.data.valMap[p];
        return v != null ? parseFloat(v) : null;
      });
      barSeries.push({
        name: m.name, type: 'bar',
        data: vals,
        barMaxWidth: 20, yAxisIndex: 0,
      });
    });

    cfg.lines.forEach(function (kw) {
      var m = findMetric(metricMap, kw);
      if (!m) return;
      hasLines = true;
      lineSeries.push({
        name: m.name, type: 'line', yAxisIndex: 1,
        data: periods.map(function (p) {
          var v = m.data.valMap[p];
          return v != null ? parseFloat(v) : null;
        }),
        showSymbol: true, symbolSize: 5, smooth: false,
        lineStyle: { width: 2 },
      });
    });

    if (!barSeries.length && !lineSeries.length) return;

    var yAxes = [
      { scale: true, splitLine: { lineStyle: { color: '#282828' } },
        axisLabel: { color: '#677179', fontSize: 10, formatter: fmtAxisVal } }
    ];
    if (hasLines) {
      yAxes.push({
        scale: true, position: 'right', splitLine: { show: false },
        axisLabel: { color: '#677179', fontSize: 10, formatter: function (v) { return v.toFixed(1) + '%'; } }
      });
    }

    var legendNames = barSeries.concat(lineSeries).map(function (s) { return s.name; });

    var c = echarts.init(el, 'lb', { renderer: 'canvas' });
    c.setOption({
      color: cfg.colors,
      animationDuration: 600, animationEasing: 'cubicOut',
      tooltip: { trigger: 'axis', axisPointer: { type: 'shadow' }, backgroundColor: '#0e0e0e',
        borderColor: '#282828', textStyle: { color: '#feffff', fontSize: 11 },
        formatter: function (params) {
          var s = params[0].axisValue + '<br/>';
          params.forEach(function (p) {
            if (p.value == null) return;
            var v = typeof p.value === 'object' ? p.value.value : p.value;
            var isPercent = p.seriesIndex >= barSeries.length;
            s += p.marker + p.seriesName + ': ' + (isPercent ? v.toFixed(2) + '%' : fmtAxisVal(v)) + '<br/>';
          });
          return s;
        }
      },
      legend: { data: legendNames, top: 4, right: 8, textStyle: { color: '#677179', fontSize: 10 } },
      grid: { left: 64, right: hasLines ? 64 : 16, top: 36, bottom: 48 },
      xAxis: { type: 'category', data: periods,
        axisLabel: { color: '#677179', fontSize: 10, rotate: 30 },
        axisLine: { lineStyle: { color: '#282828' } } },
      yAxis: yAxes,
      dataZoom: [{ type: 'inside' }, { bottom: 4, height: 20, borderColor: '#282828' }],
      series: barSeries.concat(lineSeries),
    });
    window.addEventListener('resize', function () { c.resize(); });
  }

  function buildTable(idx, sectionData) {
    var extracted = extractData(sectionData, 8);
    var periods = extracted.periods;
    var indicators = sectionData.indicators || [];
    if (!periods.length) return;

    var thead = document.getElementById('thead-' + idx);
    var tbody = document.getElementById('tbody-' + idx);
    if (!thead || !tbody) return;

    var hr = document.createElement('tr');
    var thMetric = document.createElement('th');
    thMetric.textContent = 'Metric';
    hr.appendChild(thMetric);
    periods.forEach(function (p) {
      var th = document.createElement('th');
      th.textContent = p;
      hr.appendChild(th);
    });
    thead.appendChild(hr);

    indicators.forEach(function (ind) {
      (ind.accounts || []).forEach(function (acc) {
        var tr = document.createElement('tr');
        var tdName = document.createElement('td');
        tdName.textContent = acc.name || '';
        tr.appendChild(tdName);
        var valMap = {};
        (acc.values || []).forEach(function (v) { if (v.period) valMap[v.period] = v.value; });
        var isPercent = !!acc.percent;
        periods.forEach(function (p) {
          var td = document.createElement('td');
          var rawV = valMap[p];
          var formatted = fmtVal(rawV, isPercent);
          td.textContent = formatted;
          if (formatted !== '-') {
            var n = parseFloat(rawV);
            if (!isNaN(n)) {
              if (n > 0) td.className = 'up';
              else if (n < 0) td.className = 'dn';
            }
          }
          tr.appendChild(td);
        });
        tbody.appendChild(tr);
      });
    });
  }

  SECTIONS.forEach(function (sec, idx) {
    var sd = list[sec.key];
    if (!sd) return;
    renderChart('chart-' + sec.key, sec.key, sd);
    buildTable(idx, sd);
  });
})();
