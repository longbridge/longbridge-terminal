var raw=__JSON__;
var dates=raw.map(function(d){return d.date});
var callVols=raw.map(function(d){return+d.call_vol});
var putVols=raw.map(function(d){return+d.put_vol});
var pcRatio=raw.map(function(d){return+d.pc_ratio});
chart.setOption({
  animationDuration:800,animationEasing:'cubicOut',
  tooltip:{trigger:'axis',axisPointer:{type:'shadow'},backgroundColor:'#0e0e0e',borderColor:'#282828',textStyle:{color:'#feffff',fontSize:11}},
  legend:{data:['Call Vol','Put Vol','P/C Ratio'],top:4,right:8},
  grid:{left:70,right:70,top:32,bottom:48},
  xAxis:{type:'category',data:dates,axisLabel:{color:'#677179',fontSize:10,rotate:30},axisLine:{lineStyle:{color:'#282828'}}},
  yAxis:[
    {scale:true,splitLine:{lineStyle:{color:'#282828'}},axisLabel:{color:'#677179',fontSize:10}},
    {name:'P/C',nameTextStyle:{color:'#677179',fontSize:10},scale:true,position:'right',
      axisLabel:{color:'#677179',fontSize:10},splitLine:{show:false}}
  ],
  dataZoom:[{type:'inside'},{bottom:4,height:24,borderColor:'#282828'}],
  series:[
    {name:'Call Vol',type:'bar',stack:'vol',data:callVols,itemStyle:{color:'#41b3a9'},barMaxWidth:16},
    {name:'Put Vol',type:'bar',stack:'vol',data:putVols,itemStyle:{color:'#d84a33'},barMaxWidth:16},
    {name:'P/C Ratio',type:'line',yAxisIndex:1,data:pcRatio,showSymbol:false,smooth:true,
      lineStyle:{color:'#ffb670',width:2}}
  ]
});
buildTable(
  ['Date','Call Vol','Put Vol','P/C Ratio'],
  raw.map(function(d){return[d.date,Number(d.call_vol).toLocaleString(),Number(d.put_vol).toLocaleString(),d.pc_ratio]})
);
