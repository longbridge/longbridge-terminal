var raw=__JSON__;
var cats=['Large','Medium','Small'];
var inflow=[+raw.capital_in.large,+raw.capital_in.medium,+raw.capital_in.small];
var outflow=[+raw.capital_out.large,+raw.capital_out.medium,+raw.capital_out.small];
chart.setOption({
  animationDuration:800,animationEasing:'cubicOut',
  tooltip:{trigger:'axis',axisPointer:{type:'shadow'},backgroundColor:'#0e0e0e',borderColor:'#282828',textStyle:{color:'#feffff',fontSize:11}},
  legend:{data:['Inflow','Outflow'],top:4,right:8},
  grid:{left:70,right:16,top:36,bottom:16},
  xAxis:{type:'category',data:cats,axisLabel:{color:'#677179',fontSize:10},axisLine:{lineStyle:{color:'#282828'}}},
  yAxis:{scale:true,splitLine:{lineStyle:{color:'#282828'}},axisLabel:{color:'#677179',fontSize:10}},
  series:[
    {name:'Inflow',type:'bar',data:inflow,itemStyle:{color:'#41b3a9'},barMaxWidth:40,
      emphasis:{itemStyle:{shadowBlur:8,shadowColor:'rgba(65,179,169,0.4)'}}},
    {name:'Outflow',type:'bar',data:outflow,itemStyle:{color:'#d84a33'},barMaxWidth:40,
      emphasis:{itemStyle:{shadowBlur:8,shadowColor:'rgba(216,74,51,0.4)'}}}
  ]
});
buildTable(
  ['Direction','Large','Medium','Small'],
  [
    ['Inflow',raw.capital_in.large,raw.capital_in.medium,raw.capital_in.small],
    ['Outflow',raw.capital_out.large,raw.capital_out.medium,raw.capital_out.small]
  ]
);
