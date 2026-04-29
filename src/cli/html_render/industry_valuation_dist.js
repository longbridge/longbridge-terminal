(function(){
var raw=__JSON__;
var metricKeys=['pe','pb','ps'];
var metricLabels=['PE','PB','PS'];
// Filter to only metrics present in the data
var available=metricKeys.filter(function(k){return raw[k]&&raw[k].value;});
if(!available.length){chart.setOption({title:{text:'No data',left:'center',top:'center',textStyle:{color:'#677179'}}});return;}
var yLabels=available.map(function(k){return metricLabels[metricKeys.indexOf(k)];});
var lowData=available.map(function(k){return parseFloat(raw[k].low)||0;});
var medianData=available.map(function(k){return parseFloat(raw[k].median)||0;});
var currentData=available.map(function(k){return parseFloat(raw[k].value)||0;});
var highData=available.map(function(k){return parseFloat(raw[k].high)||0;});
chart.setOption({
  animationDuration:800,
  tooltip:{
    trigger:'axis',
    axisPointer:{type:'shadow'},
    backgroundColor:'#0e0e0e',borderColor:'#282828',
    textStyle:{color:'#feffff',fontSize:11}
  },
  legend:{
    data:['Low','Median','Current','High'],
    top:4,right:8,
    textStyle:{color:'#677179',fontSize:10}
  },
  grid:{left:70,right:30,top:40,bottom:20},
  xAxis:{
    type:'value',scale:true,
    splitLine:{lineStyle:{color:'#282828'}},
    axisLabel:{color:'#677179',fontSize:10}
  },
  yAxis:{
    type:'category',data:yLabels,
    axisLabel:{color:'#677179',fontSize:11},
    axisLine:{lineStyle:{color:'#282828'}}
  },
  series:[
    {
      name:'Low',type:'bar',data:lowData,barMaxWidth:20,
      itemStyle:{color:'rgba(100,130,180,0.55)'}
    },
    {
      name:'Median',type:'bar',data:medianData,barMaxWidth:20,
      itemStyle:{color:'rgba(100,180,150,0.55)'}
    },
    {
      name:'Current',type:'bar',data:currentData,barMaxWidth:20,
      itemStyle:{color:'#4D7FFF'},
      label:{show:true,position:'right',color:'#feffff',fontSize:10,
        formatter:function(p){return p.value.toFixed(2)+'x';}}
    },
    {
      name:'High',type:'bar',data:highData,barMaxWidth:20,
      itemStyle:{color:'rgba(180,100,100,0.45)'}
    }
  ]
});
// Build distribution table
var fmtNum=function(v){var n=parseFloat(v);return isNaN(n)?'-':n.toFixed(2)+'x';};
var fmtPct=function(v){var n=parseFloat(v);return isNaN(n)?'-':(n*100).toFixed(1)+'%';};
buildTable(
  ['Metric','Current','Low','Median','High','Rank','Percentile'],
  available.map(function(k){
    var d=raw[k];
    var ri=d.rank_index||'-';var rt=d.rank_total||'-';
    var rank=(ri!=='-'&&rt!=='-')?ri+'/'+rt:'-';
    return[metricLabels[metricKeys.indexOf(k)],fmtNum(d.value),fmtNum(d.low),fmtNum(d.median),fmtNum(d.high),rank,fmtPct(d.ranking)];
  })
);
})();
