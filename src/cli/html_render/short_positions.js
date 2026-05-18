var raw=__JSON__;
var dates=raw.map(function(d){return d.date});
var rates=raw.map(function(d){return+d.rate});
var shares=raw.map(function(d){return+d.short_shares});
chart.setOption({
  animationDuration:800,animationEasing:'cubicOut',
  tooltip:{trigger:'axis',axisPointer:{type:'cross'},backgroundColor:'#0e0e0e',borderColor:'#282828',textStyle:{color:'#feffff',fontSize:11}},
  legend:{data:['Short Rate%','Short Shares'],top:4,right:8},
  grid:{left:60,right:70,top:32,bottom:48},
  xAxis:{type:'category',data:dates,axisLabel:{color:'#677179',fontSize:10,rotate:30},axisLine:{lineStyle:{color:'#282828'}}},
  yAxis:[
    {name:'Rate%',nameTextStyle:{color:'#677179',fontSize:10},scale:true,
      splitLine:{lineStyle:{color:'#282828'}},axisLabel:{color:'#677179',fontSize:10}},
    {name:'Shares',nameTextStyle:{color:'#677179',fontSize:10},scale:true,position:'right',
      axisLabel:{color:'#677179',fontSize:10},splitLine:{show:false}}
  ],
  dataZoom:[{type:'inside'},{bottom:4,height:24,borderColor:'#282828'}],
  series:[
    {name:'Short Rate%',type:'line',yAxisIndex:0,data:rates,showSymbol:false,smooth:true,
      lineStyle:{color:'#d84a33',width:2},
      areaStyle:{color:{type:'linear',x:0,y:0,x2:0,y2:1,
        colorStops:[{offset:0,color:'rgba(216,74,51,0.2)'},{offset:1,color:'rgba(216,74,51,0)'}]
      }}},
    {name:'Short Shares',type:'bar',yAxisIndex:1,data:shares,
      itemStyle:{color:'rgba(65,179,169,0.45)'},barMaxWidth:12}
  ]
});
buildTable(
  ['Date','Short Rate%','Short Shares'],
  raw.map(function(d){return[d.date,d.rate.toFixed(2)+'%',Number(d.short_shares).toLocaleString()]})
);
