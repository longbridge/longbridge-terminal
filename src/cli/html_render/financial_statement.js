(function () {
  var raw = __JSON__;
  var periods = raw.list || [];
  if (!periods.length) return;

  var report = raw.report || '';
  function periodLabel(p) {
    var yr = p.ff_year || '';
    var per = p.ff_period != null ? String(p.ff_period) : '';
    if (report === 'af') return 'FY' + yr;
    if (report === 'saf') return 'H' + per + ' ' + yr;
    return 'Q' + per + ' ' + yr;
  }

  // Periods are newest-first; reverse for chart (oldest → newest on x-axis)
  var cols = periods.slice(0, 8).map(periodLabel);
  var colsAsc = cols.slice().reverse();

  var template = periods[0].fields || [];

  function fmtAxisVal(n) {
    var abs = Math.abs(n);
    if (abs >= 1e12) return (n / 1e12).toFixed(1) + 'T';
    if (abs >= 1e9)  return (n / 1e9).toFixed(1) + 'B';
    if (abs >= 1e6)  return (n / 1e6).toFixed(1) + 'M';
    if (abs >= 1e3)  return (n / 1e3).toFixed(1) + 'K';
    return n;
  }

  function fmtVal(v, vtype) {
    if (v === null || v === undefined || v === '') return '-';
    var n = parseFloat(v);
    if (isNaN(n)) return String(v);
    if (vtype === 'percentage') return n.toFixed(2) + '%';
    var abs = Math.abs(n);
    if (abs >= 1e12) return (n / 1e12).toFixed(2) + 'T';
    if (abs >= 1e9)  return (n / 1e9).toFixed(2) + 'B';
    if (abs >= 1e6)  return (n / 1e6).toFixed(2) + 'M';
    if (abs >= 1e3)  return (n / 1e3).toFixed(2) + 'K';
    return n.toFixed(4).replace(/\.?0+$/, '');
  }

  // Build field value map: id -> values per period (newest-first order)
  var fieldMap = {};
  periods.slice(0, 8).forEach(function (p) {
    (p.fields || []).forEach(function (f) {
      if (!fieldMap[f.id]) fieldMap[f.id] = [];
      fieldMap[f.id].push({ value: f.value, vtype: f.value_type });
    });
  });

  // Pick level-2 bignumber fields with at least one non-zero value for bars
  // Pick level-2 percentage fields for lines
  var barFields = [];
  var lineFields = [];
  template.forEach(function (f) {
    if (f.level !== 2) return;
    var vals = (fieldMap[f.id] || []).map(function (e) { return parseFloat(e.value); }).filter(function (n) { return !isNaN(n) && n !== 0; });
    if (!vals.length) return;
    if (f.value_type === 'percentage') {
      lineFields.push(f);
    } else {
      barFields.push(f);
    }
  });
  // Limit bar series to avoid clutter
  barFields = barFields.slice(0, 4);
  lineFields = lineFields.slice(0, 2);

  var hasLines = lineFields.length > 0;

  function makeSeriesData(f) {
    var entries = (fieldMap[f.id] || []).slice();
    // reverse to get oldest→newest
    entries.reverse();
    return entries.map(function (e) {
      var n = parseFloat(e.value);
      return isNaN(n) ? null : n;
    });
  }

  var barSeries = barFields.map(function (f) {
    return { name: f.name, type: 'bar', data: makeSeriesData(f), barMaxWidth: 20, yAxisIndex: 0 };
  });
  var lineSeries = lineFields.map(function (f) {
    return { name: f.name, type: 'line', data: makeSeriesData(f), yAxisIndex: 1,
      showSymbol: true, symbolSize: 4, smooth: false, lineStyle: { width: 2 } };
  });

  var chartEl = document.getElementById('fs-chart');
  if (chartEl && (barSeries.length || lineSeries.length)) {
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
    var c = echarts.init(chartEl, 'lb', { renderer: 'canvas' });
    c.setOption({
      animationDuration: 600, animationEasing: 'cubicOut',
      tooltip: { trigger: 'axis', axisPointer: { type: 'shadow' }, backgroundColor: '#0e0e0e',
        borderColor: '#282828', textStyle: { color: '#feffff', fontSize: 11 },
        formatter: function (params) {
          var s = params[0].axisValue + '<br/>';
          params.forEach(function (p) {
            if (p.value == null) return;
            var isLine = p.seriesIndex >= barSeries.length;
            s += p.marker + p.seriesName + ': ' + (isLine ? p.value.toFixed(2) + '%' : fmtAxisVal(p.value)) + '<br/>';
          });
          return s;
        }
      },
      legend: { data: legendNames, top: 4, right: 8, textStyle: { color: '#677179', fontSize: 10 } },
      grid: { left: 64, right: hasLines ? 64 : 16, top: 36, bottom: 48 },
      xAxis: { type: 'category', data: colsAsc,
        axisLabel: { color: '#677179', fontSize: 10 },
        axisLine: { lineStyle: { color: '#282828' } } },
      yAxis: yAxes,
      dataZoom: [{ type: 'inside' }, { bottom: 4, height: 20, borderColor: '#282828' }],
      series: barSeries.concat(lineSeries),
    });
    window.addEventListener('resize', function () { c.resize(); });
  }

  // Build pivot table
  var thead = document.getElementById('fs-thead');
  var tbody = document.getElementById('fs-tbody');
  if (!thead || !tbody) return;

  var hr = document.createElement('tr');
  ['Metric'].concat(cols).concat(['YoY']).forEach(function (h) {
    var th = document.createElement('th');
    th.textContent = h;
    hr.appendChild(th);
  });
  thead.appendChild(hr);

  template.forEach(function (f) {
    var tr = document.createElement('tr');
    var tdName = document.createElement('td');
    var indent = f.level === 1 ? '' : f.level === 2 ? '  ' : '    ';
    var isSection = f.level === 1 && (!f.value || f.value === '');
    if (isSection) {
      var strong = document.createElement('strong');
      strong.textContent = f.name || '';
      tdName.appendChild(strong);
    } else {
      tdName.textContent = indent + (f.name || '');
    }
    tr.appendChild(tdName);

    var entries = fieldMap[f.id] || [];
    entries.forEach(function (e, i) {
      var td = document.createElement('td');
      var v = e.value;
      if (!v || v === '' || isSection) {
        td.textContent = '';
      } else {
        var formatted = fmtVal(v, e.vtype);
        td.textContent = formatted;
        if (formatted !== '-') {
          var n = parseFloat(v);
          if (!isNaN(n)) {
            if (n > 0) td.className = 'up';
            else if (n < 0) td.className = 'dn';
          }
        }
      }
      tr.appendChild(td);
    });

    // YoY from first period's field
    var yoyTd = document.createElement('td');
    var firstField = (periods[0].fields || []).find(function (ff) { return ff.id === f.id; });
    if (firstField && firstField.yoy && !isSection) {
      var yoyN = parseFloat(firstField.yoy) * 100;
      if (!isNaN(yoyN)) {
        yoyTd.textContent = (yoyN >= 0 ? '+' : '') + yoyN.toFixed(1) + '%';
        yoyTd.className = yoyN >= 0 ? 'up' : 'dn';
      }
    }
    tr.appendChild(yoyTd);

    tbody.appendChild(tr);
  });
})();
