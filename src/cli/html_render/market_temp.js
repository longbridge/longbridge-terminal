var raw=__JSON__;
var dates=raw.map(function(d){return d.time});
var temps=raw.map(function(d){return+d.temperature});
var vals=raw.map(function(d){return+d.valuation});
var sents=raw.map(function(d){return+d.sentiment});
chart.setOption({
  animationDuration:1000,animationEasing:'cubicOut',
  tooltip:{trigger:'axis',backgroundColor:'#0e0e0e',borderColor:'#282828',textStyle:{color:'#feffff',fontSize:11}},
  legend:{data:['Temperature','Valuation','Sentiment'],top:4,right:8},
  grid:{left:50,right:16,top:32,bottom:48},
  xAxis:{type:'category',data:dates,axisLabel:{color:'#677179',fontSize:10},axisLine:{lineStyle:{color:'#282828'}}},
  yAxis:{scale:true,splitLine:{lineStyle:{color:'#282828'}},axisLabel:{color:'#677179',fontSize:10}},
  dataZoom:[{type:'inside'},{bottom:4,height:24,borderColor:'#282828'}],
  series:[
    {name:'Temperature',type:'line',data:temps,showSymbol:false,smooth:true,
      lineStyle:{color:'#d84a33',width:2},
      areaStyle:{color:{type:'linear',x:0,y:0,x2:0,y2:1,
        colorStops:[{offset:0,color:'rgba(216,74,51,0.2)'},{offset:1,color:'rgba(216,74,51,0)'}]
      }}},
    {name:'Valuation',type:'line',data:vals,showSymbol:false,smooth:true,
      lineStyle:{color:'#41b3a9',width:2}},
    {name:'Sentiment',type:'line',data:sents,showSymbol:false,smooth:true,
      lineStyle:{color:'#ffb670',width:2}}
  ]
});
buildTable(
  ['Time','Temperature','Valuation','Sentiment'],
  raw.map(function(d){return[d.time,d.temperature,d.valuation,d.sentiment]})
);
