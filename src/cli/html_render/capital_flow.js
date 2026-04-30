var raw=__JSON__;
var dates=raw.map(function(d){return d.time});
var vals=raw.map(function(d){return+d.inflow});
chart.setOption({
  animationDuration:800,animationEasing:'cubicOut',
  tooltip:{trigger:'axis',backgroundColor:'#0e0e0e',borderColor:'#282828',textStyle:{color:'#feffff',fontSize:11}},
  grid:{left:70,right:16,top:16,bottom:48},
  xAxis:{type:'category',data:dates,axisLabel:{color:'#677179',fontSize:10},axisLine:{lineStyle:{color:'#282828'}}},
  yAxis:{scale:true,splitLine:{lineStyle:{color:'#282828'}},axisLabel:{color:'#677179',fontSize:10}},
  dataZoom:[{type:'inside'},{bottom:4,height:24,borderColor:'#282828'}],
  series:[{
    name:'Capital Inflow',type:'bar',
    data:vals.map(function(v){return{value:v,itemStyle:{color:v>=0?'#41b3a9':'#d84a33'}}}),
    barMaxWidth:16,
    emphasis:{itemStyle:{shadowBlur:8,shadowColor:'rgba(0,0,0,0.5)'}}
  }]
});
buildTable(
  ['Time','Inflow'],
  raw.map(function(d){return[d.time,d.inflow]})
);
