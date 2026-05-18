(function(){
var raw=__JSON__;
var metrics=raw.metrics||{};
var keys=Object.keys(metrics);
if(!keys.length){chart.setOption({title:{text:'No data',left:'center',top:'center',textStyle:{color:'#677179'}}});return;}
var dateSet={};
var seriesMap={};
keys.forEach(function(k){
  seriesMap[k]={};
  (metrics[k].list||[]).forEach(function(pt){
    var d=new Date(+pt.timestamp*1000).toISOString().slice(0,10);
    dateSet[d]=1;
    seriesMap[k][d]=pt.value;
  });
});
var dates=Object.keys(dateSet).sort();
var colors=['#4488CC','#44AA66','#D94400','#0088AA','#CC4488','#D4BC00'];
var series=keys.map(function(k,i){
  return{name:k.toUpperCase(),type:'line',
    data:dates.map(function(d){return seriesMap[k][d]!=null?+seriesMap[k][d]:null}),
    showSymbol:false,smooth:false,connectNulls:false,
    lineStyle:{color:colors[i%colors.length],width:2}};
});
chart.setOption({
  animationDuration:800,animationEasing:'cubicOut',
  tooltip:{trigger:'axis',backgroundColor:'#0e0e0e',borderColor:'#282828',textStyle:{color:'#feffff',fontSize:11}},
  legend:{data:keys.map(function(k){return k.toUpperCase()}),top:4,right:8,textStyle:{color:'#677179'}},
  grid:{left:60,right:16,top:32,bottom:48},
  xAxis:{type:'category',data:dates,axisLabel:{color:'#677179',fontSize:10},axisLine:{lineStyle:{color:'#282828'}}},
  yAxis:{scale:true,splitLine:{lineStyle:{color:'#282828'}},axisLabel:{color:'#677179',fontSize:10}},
  dataZoom:[{type:'inside'},{bottom:4,height:24,borderColor:'#282828'}],
  series:series
});
buildTable(
  ['Date'].concat(keys.map(function(k){return k.toUpperCase()})),
  dates.map(function(d){
    return[d].concat(keys.map(function(k){
      var v=seriesMap[k][d];return v!=null?v:'';
    }));
  }));
})();
