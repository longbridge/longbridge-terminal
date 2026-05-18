var raw=__JSON__;
var asks=(raw.asks||[]).slice().reverse();
var bids=raw.bids||[];
var askPrices=asks.map(function(d){return d.price});
var bidPrices=bids.map(function(d){return d.price});
var allPrices=askPrices.concat(bidPrices);
var askMap={};asks.forEach(function(d){askMap[d.price]=+d.volume});
var bidMap={};bids.forEach(function(d){bidMap[d.price]=+d.volume});
chart.setOption({
  animationDuration:600,animationEasing:'cubicOut',
  tooltip:{trigger:'axis',axisPointer:{type:'shadow'},backgroundColor:'#0e0e0e',borderColor:'#282828',
    textStyle:{color:'#feffff',fontSize:11},
    formatter:function(p){return p.map(function(s){return s.seriesName+': '+Math.abs(s.value).toLocaleString()}).join('<br>')}},
  legend:{data:['Ask','Bid'],top:4,right:8},
  grid:{left:70,right:16,top:36,bottom:16},
  xAxis:{type:'value',axisLabel:{color:'#677179',fontSize:10,formatter:function(v){return Math.abs(v).toLocaleString()}},
    splitLine:{lineStyle:{color:'#282828'}}},
  yAxis:{type:'category',data:allPrices,axisLabel:{color:'#677179',fontSize:10}},
  series:[
    {name:'Ask',type:'bar',data:allPrices.map(function(p){return askMap[p]||null}),
      itemStyle:{color:'#d84a33'},barMaxWidth:20,
      emphasis:{itemStyle:{shadowBlur:8,shadowColor:'rgba(216,74,51,0.5)'}}},
    {name:'Bid',type:'bar',data:allPrices.map(function(p){return bidMap[p]?-bidMap[p]:null}),
      itemStyle:{color:'#41b3a9'},barMaxWidth:20,
      emphasis:{itemStyle:{shadowBlur:8,shadowColor:'rgba(65,179,169,0.5)'}}}
  ]
});
var rows=[];
var maxLen=Math.max(asks.length,bids.length);
for(var i=0;i<maxLen;i++){
  var a=asks[i]||{};var b=bids[i]||{};
  rows.push([a.price||'',a.volume?Number(a.volume).toLocaleString():'',
             b.price||'',b.volume?Number(b.volume).toLocaleString():'']);
}
buildTable(['Ask Price','Ask Vol','Bid Price','Bid Vol'],rows);
