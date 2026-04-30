var raw=__JSON__;
var analyst=raw.analyst||{};
var inst=raw.instratings||{};
var ev=inst.evaluate||{};
var cats=['Strong Buy','Buy','Hold','Sell','Under'];
var vals=[+ev.strong_buy||0,+ev.buy||0,+ev.hold||0,+ev.sell||0,+ev.under||0];
var colors=['#5da602','#41b3a9','#7A7A8E','#D94400','#d84a33'];
(function(){
  var el=document.getElementById('ir-consensus');
  if(!el)return;
  var tgt=(+inst.target).toFixed(2);
  var chg=(+inst.change).toFixed(2);
  var rec=(inst.recommend||'').replace(/_/g,' ').replace(/\b\w/g,function(c){return c.toUpperCase()});
  var at=inst.instratings_updated_at||inst.updated_at||'';
  el.innerHTML=[
    ['Recommendation',rec],
    ['Target Price',tgt||'-'],
    ['Change',chg?chg+'%':'-'],
    ['Updated',at||'-'],
  ].map(function(pair){
    var valClass=pair[0]==='Recommendation'?(+ev.strong_buy>(+ev.sell+ev.under)?'text-[#5da602]':''):'';
    return '<div class="flex flex-col gap-0.5"><span class="text-[10px] text-[#677179] uppercase tracking-wider">'+pair[0]+'</span>'
      +'<span class="font-mono text-sm '+valClass+'">'+pair[1]+'</span></div>';
  }).join('');
})();
chart.setOption({
  animationDuration:600,animationEasing:'cubicOut',
  tooltip:{trigger:'axis',axisPointer:{type:'shadow'},backgroundColor:'#0e0e0e',borderColor:'#282828',textStyle:{color:'#feffff',fontSize:11}},
  legend:{data:cats,top:4,right:8,textStyle:{color:'#677179'}},
  grid:{left:96,right:16,top:36,bottom:16},
  xAxis:{type:'value',axisLabel:{color:'#677179',fontSize:10},splitLine:{lineStyle:{color:'#282828'}}},
  yAxis:{type:'category',data:cats,axisLabel:{color:'#677179',fontSize:10}},
  series:[{
    name:'Ratings',type:'bar',
    data:vals.map(function(v,i){return{value:v,itemStyle:{color:colors[i]}}}),
    barMaxWidth:24,label:{show:true,position:'right',color:'#677179',fontSize:10}
  }]
});
var tgt=analyst.target||{};
buildTable(
  ['Category','Count','Lowest Target','Highest Target','Prev Close'],
  cats.map(function(c,i){
    return[c,vals[i],i===0?tgt.lowest_price||'-':'',i===0?tgt.highest_price||'-':'',i===0?tgt.prev_close||'-':''];
  })
);
