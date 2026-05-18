var raw=__JSON__;
var items=raw.klines||raw.minutes||[];
var dates=items.map(function(d){
  var ts=+d.timestamp;
  var dt=new Date(ts*1000);
  return dt.toISOString().slice(0,10);
});
var premiums=items.map(function(d){return+(+d.ahpremium_rate*100).toFixed(4)});
var aprices=items.map(function(d){return+d.aprice});
var hprices=items.map(function(d){return+d.hprice});
chart.setOption({
  animationDuration:800,animationEasing:'cubicOut',
  tooltip:{trigger:'axis',backgroundColor:'#0e0e0e',borderColor:'#282828',textStyle:{color:'#feffff',fontSize:11},
    formatter:function(params){
      var s=params[0].axisValue+'<br/>';
      params.forEach(function(p){s+=p.marker+p.seriesName+': '+p.value+'<br/>'});
      return s;
    }},
  legend:{data:['Premium%','A-share(CNY)','H-share(HKD)'],top:4,right:8,textStyle:{color:'#677179'}},
  grid:[{left:64,right:72,top:32,bottom:48}],
  xAxis:[{type:'category',data:dates,axisLabel:{color:'#677179',fontSize:10,rotate:30},axisLine:{lineStyle:{color:'#282828'}}}],
  yAxis:[
    {name:'Premium%',nameTextStyle:{color:'#677179',fontSize:10},scale:true,
      splitLine:{lineStyle:{color:'#282828'}},axisLabel:{color:'#677179',fontSize:10,formatter:function(v){return v+'%'}}},
    {name:'Price',nameTextStyle:{color:'#677179',fontSize:10},scale:true,position:'right',
      axisLabel:{color:'#677179',fontSize:10},splitLine:{show:false}}
  ],
  dataZoom:[{type:'inside'},{bottom:4,height:24,borderColor:'#282828'}],
  series:[
    {name:'Premium%',type:'line',yAxisIndex:0,data:premiums,showSymbol:false,
      lineStyle:{color:'#D94400',width:2},
      areaStyle:{color:{type:'linear',x:0,y:0,x2:0,y2:1,
        colorStops:[{offset:0,color:'rgba(255,144,0,0.25)'},{offset:1,color:'rgba(255,144,0,0)'}]
      }}},
    {name:'A-share(CNY)',type:'line',yAxisIndex:1,data:aprices,showSymbol:false,
      lineStyle:{color:'#4488CC',width:1.5}},
    {name:'H-share(HKD)',type:'line',yAxisIndex:1,data:hprices,showSymbol:false,
      lineStyle:{color:'#44AA66',width:1.5}}
  ]
});
buildTable(
  ['Date','Premium%','A-share(CNY)','H-share(HKD)','FX Rate'],
  items.map(function(d){
    var ts=+d.timestamp;
    var dt=new Date(ts*1000).toISOString().slice(0,10);
    return[dt,(+d.ahpremium_rate*100).toFixed(2)+'%',d.aprice,d.hprice,d.currency_rate];
  }));
