var raw=__JSON__;
var dates=raw.map(function(d){return d.time});
var prices=raw.map(function(d){return+d.price});
var avgPrices=raw.map(function(d){return+d.avg_price});
var vols=raw.map(function(d){return+d.volume});
chart.setOption({
  animationDuration:800,animationEasing:'cubicOut',
  tooltip:{trigger:'axis',axisPointer:{type:'cross'},backgroundColor:'#0e0e0e',borderColor:'#282828',textStyle:{color:'#feffff',fontSize:11}},
  legend:{data:['Price','Avg Price','Volume'],top:4,right:8},
  grid:[
    {left:60,right:16,top:32,bottom:52},
    {left:60,right:16,top:'74%',bottom:32}
  ],
  xAxis:[
    {type:'category',data:dates,gridIndex:0,boundaryGap:false,
      axisLine:{lineStyle:{color:'#282828'}},axisLabel:{color:'#677179',fontSize:10}},
    {type:'category',data:dates,gridIndex:1,axisLabel:{show:false},
      axisLine:{lineStyle:{color:'#282828'}}}
  ],
  yAxis:[
    {scale:true,gridIndex:0,splitLine:{lineStyle:{color:'#282828'}},axisLabel:{color:'#677179',fontSize:10}},
    {scale:true,gridIndex:1,splitLine:{lineStyle:{color:'#282828'}},axisLabel:{color:'#677179',fontSize:10}}
  ],
  dataZoom:[{type:'inside',xAxisIndex:[0,1]}],
  series:[
    {name:'Price',type:'line',xAxisIndex:0,yAxisIndex:0,data:prices,showSymbol:false,smooth:true,
      lineStyle:{color:'#41b3a9',width:2},
      areaStyle:{color:{type:'linear',x:0,y:0,x2:0,y2:1,
        colorStops:[{offset:0,color:'rgba(65,179,169,0.25)'},{offset:1,color:'rgba(65,179,169,0)'}]
      }},
      emphasis:{focus:'series'} },
    {name:'Avg Price',type:'line',xAxisIndex:0,yAxisIndex:0,data:avgPrices,showSymbol:false,smooth:true,
      lineStyle:{color:'#ffb670',width:1.5,type:'dashed'}},
    {name:'Volume',type:'bar',xAxisIndex:1,yAxisIndex:1,data:vols,
      itemStyle:{color:'rgba(65,179,169,0.4)'},barMaxWidth:6}
  ]
});
buildTable(
  ['Time','Price','Avg Price','Volume'],
  raw.map(function(d){return[d.time,d.price,d.avg_price,Number(d.volume).toLocaleString()]})
);
