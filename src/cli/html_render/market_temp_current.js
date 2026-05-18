var cur=__CURRENT_JSON__;
var hist=__HISTORY_JSON__;
(function(){
  var descEl=document.getElementById('desc');
  if(descEl&&cur.description){
    var t=+cur.temperature;
    descEl.textContent=cur.description;
    descEl.style.color=t<30?'#41b3a9':t<70?'#aa7900':'#d84a33';
  }
  function mkGauge(el,val){
    var c=echarts.init(document.getElementById(el),'lb',{renderer:'canvas'});
    c.setOption({series:[{
      type:'gauge',min:0,max:100,
      radius:'80%',center:['50%','58%'],
      startAngle:210,endAngle:-30,
      axisLine:{lineStyle:{width:16,color:[[0.3,'#41b3a9'],[0.7,'#aa7900'],[1,'#d84a33']]}},
      axisTick:{show:false},splitLine:{show:false},axisLabel:{show:false},
      pointer:{width:5,length:'65%',itemStyle:{color:'#feffff',opacity:0.9}},
      detail:{formatter:'{value}',fontSize:28,fontWeight:'bold',color:'#feffff',offsetCenter:[0,'28%']},
      title:{show:false},
      data:[{value:+val}]
    }]});
    window.addEventListener('resize',function(){c.resize()});
  }
  mkGauge('g1',cur.temperature);
  mkGauge('g2',cur.valuation);
  mkGauge('g3',cur.sentiment);
  if(!hist||hist.length===0){
    var s=document.getElementById('hist-section');
    if(s)s.style.display='none';
    return;
  }
  var hc=echarts.init(document.getElementById('hist'),'lb',{renderer:'canvas'});
  var dates=hist.map(function(d){return d.time});
  var temps=hist.map(function(d){return+d.temperature});
  var vals=hist.map(function(d){return+d.valuation});
  var sents=hist.map(function(d){return+d.sentiment});
  hc.setOption({
    animationDuration:1000,animationEasing:'cubicOut',
    tooltip:{trigger:'axis',backgroundColor:'#0e0e0e',borderColor:'#282828',textStyle:{color:'#feffff',fontSize:11}},
    legend:{data:['Temperature','Valuation','Sentiment'],top:4,right:8},
    grid:{left:50,right:16,top:32,bottom:48},
    xAxis:{type:'category',data:dates,axisLabel:{color:'#677179',fontSize:10},axisLine:{lineStyle:{color:'#282828'}}},
    yAxis:{scale:true,splitLine:{lineStyle:{color:'#282828'}},axisLabel:{color:'#677179',fontSize:10}},
    dataZoom:[{type:'inside'},{bottom:4,height:24,borderColor:'#282828'}],
    series:[
      {name:'Temperature',type:'line',data:temps,showSymbol:false,smooth:true,
        lineStyle:{color:'#d84a33',width:2},
        areaStyle:{color:{type:'linear',x:0,y:0,x2:0,y2:1,
          colorStops:[{offset:0,color:'rgba(216,74,51,0.2)'},{offset:1,color:'rgba(216,74,51,0)'}]
        }}},
      {name:'Valuation',type:'line',data:vals,showSymbol:false,smooth:true,
        lineStyle:{color:'#41b3a9',width:2}},
      {name:'Sentiment',type:'line',data:sents,showSymbol:false,smooth:true,
        lineStyle:{color:'#ffb670',width:2}}
    ]
  });
  window.addEventListener('resize',function(){hc.resize()});
  buildTable(
    ['Time','Temperature','Valuation','Sentiment'],
    hist.map(function(d){return[d.time,d.temperature,d.valuation,d.sentiment]}));
})();
