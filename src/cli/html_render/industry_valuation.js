(function(){
var raw=__JSON__;
var items=raw.list||[];
if(!items.length){chart.setOption({title:{text:'No data',left:'center',top:'center',textStyle:{color:'#677179'}}});return;}
var metrics=['pe','pb','ps','roe','div_yld'];
var labels=['PE','PB','PS','ROE','Div Yld'];
// Compute min/max for each metric across all items for normalization
var mins=metrics.map(function(k){
  var vals=items.map(function(d){return parseFloat(d[k])||0});
  return Math.min.apply(null,vals);
});
var maxs=metrics.map(function(k){
  var vals=items.map(function(d){return parseFloat(d[k])||0});
  return Math.max.apply(null,vals);
});
var indicator=labels.map(function(l,i){
  var mn=mins[i];
  var mx=maxs[i];
  var pad=(mx-mn)*0.15||1;
  return{name:l,min:Math.max(0,mn-pad),max:mx+pad};
});
var colors=['#4488CC','#44AA66','#D94400','#0088AA','#CC4488','#D4BC00','#8855BB','#00B89A','#7788AA','#7A7A8E'];
var series=items.map(function(d,i){
  return{
    name:d.name||d.counter_id,
    type:'radar',
    symbol:'circle',symbolSize:4,
    itemStyle:{color:colors[i%colors.length]},
    lineStyle:{color:colors[i%colors.length],width:2},
    areaStyle:{color:colors[i%colors.length].replace(')',',0.08)').replace('#','rgba(').replace(/([0-9a-fA-F]{2})/g,function(h){return parseInt(h,16)+','}).slice(0,-1)+'0.06)'},
    data:[{value:metrics.map(function(k){return parseFloat(d[k])||0}),name:d.name||d.counter_id}]
  };
});
// Fix areaStyle: use simple rgba approach
series=items.map(function(d,i){
  var c=colors[i%colors.length];
  return{
    name:d.name||d.counter_id,
    type:'radar',
    symbol:'circle',symbolSize:4,
    itemStyle:{color:c},
    lineStyle:{color:c,width:2},
    data:[{value:metrics.map(function(k){return parseFloat(d[k])||0}),name:d.name||d.counter_id}]
  };
});
chart.setOption({
  animationDuration:800,
  tooltip:{trigger:'item',backgroundColor:'#0e0e0e',borderColor:'#282828',textStyle:{color:'#feffff',fontSize:11}},
  legend:{
    data:items.map(function(d){return d.name||d.counter_id}),
    bottom:0,
    textStyle:{color:'#677179',fontSize:10},
    itemWidth:12,itemHeight:10
  },
  radar:{
    indicator:indicator,
    center:['50%','48%'],radius:'60%',
    axisName:{color:'#677179',fontSize:11},
    axisLine:{lineStyle:{color:'#282828'}},
    splitLine:{lineStyle:{color:'#282828'}},
    splitArea:{areaStyle:{color:['rgba(40,40,40,0.3)','rgba(20,20,20,0.3)']}}
  },
  series:series
});
// Build summary table
var fmtNum=function(v){var n=parseFloat(v);return isNaN(n)?'-':n.toFixed(2);};
var fmtCap=function(v){var n=parseFloat(v);if(isNaN(n)||n===0)return'-';if(n>=1e12)return(n/1e12).toFixed(2)+'T';if(n>=1e9)return(n/1e9).toFixed(2)+'B';if(n>=1e6)return(n/1e6).toFixed(2)+'M';return n.toFixed(0);};
buildTable(
  ['Symbol','Name','Market Cap','Price','PE','PB','PS','ROE','Div Yld'],
  items.map(function(d){
    var parts=(d.counter_id||'').split('/');
    var sym=parts.length>=3?parts[2]+'.'+parts[1]:d.counter_id||'-';
    return[sym,d.name||'-',fmtCap(d.market_value),fmtNum(d.price_close),fmtNum(d.pe),fmtNum(d.pb),fmtNum(d.ps),fmtNum(d.roe),fmtNum(d.div_yld)];
  })
);
})();
