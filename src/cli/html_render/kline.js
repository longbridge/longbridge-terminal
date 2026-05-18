var raw=__JSON__;
var dates=raw.map(function(d){return d.time});
var ohlc=raw.map(function(d){return[+d.open,+d.close,+d.low,+d.high]});
var vols=raw.map(function(d){return{value:+d.volume,itemStyle:{color:+d.close>=+d.open?'#5da602':'#d84a33'}}});
chart.setOption({
  animationDuration:800,animationEasing:'cubicOut',
  tooltip:{trigger:'axis',axisPointer:{type:'cross'},backgroundColor:'#0e0e0e',borderColor:'#282828',textStyle:{color:'#feffff',fontSize:11}},
  legend:{data:['K Line','Volume'],top:4,right:8},
  grid:[
    {left:60,right:16,top:32,bottom:52},
    {left:60,right:16,top:'74%',bottom:32}
  ],
  xAxis:[
    {type:'category',data:dates,scale:true,gridIndex:0,boundaryGap:true,
      axisLine:{lineStyle:{color:'#282828'}},axisLabel:{color:'#677179',fontSize:10},
      splitLine:{show:false}},
    {type:'category',data:dates,scale:true,gridIndex:1,axisLabel:{show:false},
      axisLine:{lineStyle:{color:'#282828'}}}
  ],
  yAxis:[
    {scale:true,gridIndex:0,splitLine:{lineStyle:{color:'#282828'}},
      axisLabel:{color:'#677179',fontSize:10},axisLine:{show:false}},
    {scale:true,gridIndex:1,splitLine:{lineStyle:{color:'#282828'}},
      axisLabel:{color:'#677179',fontSize:10},axisLine:{show:false}}
  ],
  dataZoom:[
    {type:'inside',xAxisIndex:[0,1],start:0,end:100},
    {xAxisIndex:[0,1],bottom:4,height:24,borderColor:'#282828'}
  ],
  series:[
    {name:'K Line',type:'candlestick',xAxisIndex:0,yAxisIndex:0,data:ohlc,
      emphasis:{itemStyle:{shadowBlur:12,shadowColor:'rgba(0,0,0,0.8)'}} },
    {name:'Volume',type:'bar',xAxisIndex:1,yAxisIndex:1,data:vols,barMaxWidth:12}
  ]
});
buildTable(
  ['Time','Open','High','Low','Close','Volume'],
  raw.map(function(d){return[d.time,d.open,d.high,d.low,d.close,Number(d.volume).toLocaleString()]})
);
