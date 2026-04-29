var raw=__JSON__;
var stats=raw.statistics||{};
var trades=(raw.trades||[]).slice().reverse();
var prices=trades.map(function(d){return d.price});
var buy=trades.map(function(d){return+d.buy_amount});
var sell=trades.map(function(d){return+d.sell_amount});
var neutral=trades.map(function(d){return+d.neutral_amount});
var total=+stats.total_amount||1;
var buyPct=(+stats.buy/total*100).toFixed(1);
var sellPct=(+stats.sell/total*100).toFixed(1);
var neutralPct=(+stats.neutral/total*100).toFixed(1);
var fmtN=function(v){var n=+v;return n>=1e6?(n/1e6).toFixed(2)+'M':n>=1e3?(n/1e3).toFixed(0)+'K':n.toLocaleString()};
(function(){
  var el=document.getElementById('ts-stats');
  if(!el)return;
  el.innerHTML=[
    ['Prev Close',stats.preclose||'-'],
    ['Avg Price',stats.avgprice||'-'],
    ['Trades',(+stats.trades_count).toLocaleString()],
    ['Buy',fmtN(stats.buy)+' ('+buyPct+'%)'],
    ['Sell',fmtN(stats.sell)+' ('+sellPct+'%)'],
    ['Neutral',fmtN(stats.neutral)+' ('+neutralPct+'%)'],
  ].map(function(pair){
    return '<div class="flex flex-col gap-0.5"><span class="text-[10px] text-[#677179] uppercase tracking-wider">'+pair[0]+'</span>'
      +'<span class="text-[#feffff] font-mono text-sm">'+pair[1]+'</span></div>';
  }).join('');
})();
var fmtAxis=function(v){return v>=1e6?(v/1e6).toFixed(1)+'M':v>=1e3?(v/1e3).toFixed(0)+'K':v};
chart.setOption({
  animationDuration:600,animationEasing:'cubicOut',
  tooltip:{trigger:'axis',axisPointer:{type:'shadow'},backgroundColor:'#0e0e0e',borderColor:'#282828',textStyle:{color:'#feffff',fontSize:11},
    formatter:function(params){
      var s=params[0].axisValue+'<br/>';
      params.forEach(function(p){s+=p.marker+p.seriesName+': '+fmtN(p.value)+'<br/>'});
      return s;
    }},
  legend:{data:['Buy','Sell','Neutral'],top:4,right:8,textStyle:{color:'#677179'}},
  grid:{left:72,right:20,top:36,bottom:16},
  xAxis:{type:'value',axisLabel:{color:'#677179',fontSize:10,formatter:fmtAxis},splitLine:{lineStyle:{color:'#282828'}}},
  yAxis:{type:'category',data:prices,axisLabel:{color:'#677179',fontSize:10}},
  series:[
    {name:'Buy',type:'bar',stack:'vol',data:buy,itemStyle:{color:'#5da602'},barMaxWidth:14},
    {name:'Sell',type:'bar',stack:'vol',data:sell,itemStyle:{color:'#d84a33'},barMaxWidth:14},
    {name:'Neutral',type:'bar',stack:'vol',data:neutral,itemStyle:{color:'#7A7A8E'},barMaxWidth:14}
  ]
});
buildTable(
  ['Price','Buy','Sell','Neutral'],
  trades.map(function(d){return[d.price,Number(d.buy_amount).toLocaleString(),Number(d.sell_amount).toLocaleString(),Number(d.neutral_amount).toLocaleString()]})
);
