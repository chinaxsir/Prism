//! 内嵌管理后台页面（GET /admin）。纯静态 HTML+原生 JS，
//! 管理密钥仅存浏览器 localStorage，所有请求携带 X-Admin-Key。

pub const PAGE: &str = r##"<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Prism 授权管理后台</title>
<style>
:root{--bg:#0f1220;--card:#1a1f36;--hover:#232a4a;--fg:#e6e9f5;--mut:#8b93b8;--acc:#5b7fff;--bad:#ff5b6a;--warn:#ffb454;--ok:#3ecf8e}
*{box-sizing:border-box;margin:0;padding:0}
body{background:var(--bg);color:var(--fg);font:14px/1.6 -apple-system,"PingFang SC","Microsoft YaHei",sans-serif;padding:24px;max-width:1100px;margin:0 auto}
h1{font-size:20px;margin-bottom:16px}
.card{background:var(--card);border-radius:12px;padding:16px;margin-bottom:16px}
.card h3{font-size:14px;margin-bottom:10px;font-weight:600}
.row{display:flex;gap:8px;flex-wrap:wrap;align-items:center}
input,select,button{background:var(--hover);border:1px solid #ffffff14;color:var(--fg);border-radius:8px;padding:8px 12px;font-size:13px}
button{cursor:pointer}button:hover{border-color:var(--acc)}
button.primary{background:var(--acc);border-color:var(--acc)}
button.danger{color:var(--bad)}
button.mini{padding:3px 8px;font-size:11px}
table{width:100%;border-collapse:collapse;font-size:12px}
th,td{text-align:left;padding:8px 6px;border-bottom:1px solid #ffffff0d;vertical-align:top}
th{color:var(--mut);font-weight:500}
.mut{color:var(--mut)}.ok{color:var(--ok)}.bad{color:var(--bad)}.warn{color:var(--warn)}
.tag{display:inline-block;padding:1px 8px;border-radius:6px;background:var(--hover);font-size:11px;margin-right:4px}
.stats{display:grid;grid-template-columns:repeat(auto-fit,minmax(130px,1fr));gap:10px}
.stat{background:var(--hover);border-radius:10px;padding:14px}
.stat b{display:block;font-size:24px}
.chart{display:flex;align-items:flex-end;gap:4px;height:100px;margin-top:10px}
.bar{flex:1;background:var(--acc);border-radius:3px 3px 0 0;min-height:2px;position:relative}
.bar:hover::after{content:attr(data-tip);position:absolute;bottom:100%;left:50%;transform:translateX(-50%);background:#000;color:#fff;font-size:11px;padding:2px 6px;border-radius:4px;white-space:nowrap}
.tabs{display:flex;gap:4px;margin-bottom:16px}
.tabs button.on{background:var(--acc)}
#toast{position:fixed;top:16px;right:16px;background:var(--card);border:1px solid var(--acc);border-radius:8px;padding:10px 16px;display:none;z-index:9}
details{margin-top:6px}summary{cursor:pointer;color:var(--acc);font-size:12px}
.editbox{background:#00000033;border-radius:8px;padding:10px;margin-top:8px}
.editbox .row{margin-bottom:6px}
.editbox input{width:110px}
#issued{margin-top:10px;display:none}
#issued pre{background:#00000044;border-radius:8px;padding:10px;font-size:12px;max-height:180px;overflow:auto;white-space:pre-wrap;word-break:break-all}
</style>
</head>
<body>
<h1>Prism 授权管理后台</h1>
<div id="login" class="card">
  <div class="row">
    <input id="key" type="password" placeholder="管理密钥 (PRISM_LICENSE_ADMIN_KEY)" style="flex:1;min-width:240px">
    <button class="primary" onclick="saveKey()">进入</button>
  </div>
</div>
<div id="main" style="display:none">
  <div class="tabs">
    <button data-t="overview" class="on">总览</button>
    <button data-t="codes">激活码</button>
    <button data-t="blacklist">黑名单</button>
    <button style="margin-left:auto" onclick="logout()">退出</button>
  </div>

  <div id="tab-overview">
    <div class="card"><h3>总量</h3><div class="stats" id="stats"></div></div>
    <div class="card"><h3>激活码存量</h3><div class="stats" id="stock"></div></div>
    <div class="card"><h3>订阅到期分布（已激活）</h3><div class="stats" id="expiry"></div></div>
    <div class="card"><h3>渠道</h3><div class="stats" id="channels"></div></div>
    <div class="card"><h3>近 14 天激活</h3><div class="chart" id="chart"></div></div>
  </div>

  <div id="tab-codes" style="display:none">
    <div class="card">
      <div class="row">
        <select id="ck"><option value="lifetime">买断</option><option value="subscription">订阅</option></select>
        <input id="cd" type="number" placeholder="天数(订阅)" style="width:100px">
        <input id="cm" type="number" placeholder="设备数" value="3" style="width:80px">
        <input id="cn" type="number" placeholder="数量" value="1" style="width:70px">
        <input id="cnote" placeholder="备注" style="width:140px">
        <input id="cemail" placeholder="绑定邮箱(可选)" style="width:170px">
        <button class="primary" onclick="issue()">批量生成</button>
      </div>
      <div id="issued">
        <div class="row" style="margin-bottom:6px">
          <b id="issued-title"></b>
          <button class="mini" onclick="copyIssued()">复制</button>
          <button class="mini" onclick="downloadIssued('csv')">下载 CSV</button>
          <button class="mini" onclick="downloadIssued('txt')">下载 TXT</button>
          <button class="mini" onclick="$('#issued').style.display='none'">收起</button>
        </div>
        <pre id="issued-list"></pre>
      </div>
    </div>
    <div class="card">
      <div class="row" style="margin-bottom:10px">
        <input id="fq" placeholder="搜索激活码 / 邮箱 / 备注" style="flex:1;min-width:200px" onkeydown="if(event.key==='Enter')loadCodes()">
        <select id="fs"><option value="">全部状态</option><option value="active">在用</option><option value="revoked">已停用</option></select>
        <button onclick="loadCodes()">搜索</button>
        <button class="mini" onclick="exportFiltered('csv')">导出 CSV</button>
        <button class="mini" onclick="exportFiltered('txt')">导出 TXT</button>
      </div>
      <table><thead><tr><th>激活码</th><th>类型</th><th>状态</th><th>设备</th><th>邮箱</th><th>创建/到期</th><th>备注</th><th>操作</th></tr></thead><tbody id="codes"></tbody></table>
    </div>
  </div>

  <div id="tab-blacklist" style="display:none">
    <div class="card"><div class="row">
      <input id="bd" placeholder="设备 ID" style="flex:1;min-width:220px">
      <input id="br" placeholder="原因(可选)" style="width:160px">
      <button class="primary" onclick="ban()">拉黑</button>
    </div></div>
    <div class="card"><table><thead><tr><th>设备 ID</th><th>原因</th><th>时间</th><th>操作</th></tr></thead><tbody id="bl"></tbody></table></div>
  </div>
</div>
<div id="toast"></div>
<script>
const $=s=>document.querySelector(s);
let KEY=localStorage.getItem('prism_admin_key')||'';
let lastCodes=[];      // 当前筛选结果
let lastIssued=null;   // 最近批量生成的码
async function api(p,opt={}){opt.headers={'X-Admin-Key':KEY,'Content-Type':'application/json'};if(opt.body&&typeof opt.body!=='string')opt.body=JSON.stringify(opt.body);
  const r=await fetch(p,opt);if(r.status===401){logout();throw new Error('密钥错误');}
  const j=await r.json().catch(()=>({}));if(!r.ok)throw new Error(j.error&&j.error.message||('HTTP '+r.status));return j;}
function toast(m,ok){const t=$('#toast');t.textContent=m;t.style.borderColor=ok===false?'var(--bad)':'var(--acc)';t.style.display='block';setTimeout(()=>t.style.display='none',2500);}
function ts(s){return s?new Date(s*1000).toLocaleDateString('zh-CN'):'-'}
function csvCell(v){v=v==null?'':String(v);return /[",\n]/.test(v)?'"'+v.replace(/"/g,'""')+'"':v;}
function download(name,text){if(name.endsWith('.csv'))text='﻿'+text;const a=document.createElement('a');a.href=URL.createObjectURL(new Blob([text],{type:'text/plain;charset=utf-8'}));a.download=name;a.click();URL.revokeObjectURL(a.href);}
function saveKey(){KEY=$('#key').value.trim();localStorage.setItem('prism_admin_key',KEY);boot();}
function logout(){localStorage.removeItem('prism_admin_key');KEY='';$('#login').style.display='';$('#main').style.display='none';}
async function boot(){if(!KEY)return;try{await api('/admin/stats');}catch(e){return}$('#login').style.display='none';$('#main').style.display='';loadOverview();loadCodes();loadBlacklist();}
document.querySelectorAll('.tabs button[data-t]').forEach(b=>b.onclick=()=>{document.querySelectorAll('.tabs button[data-t]').forEach(x=>x.classList.remove('on'));b.classList.add('on');['overview','codes','blacklist'].forEach(t=>$('#tab-'+t).style.display=t===b.dataset.t?'':'none');if(b.dataset.t==='overview')loadOverview();});

function statCards(arr){return arr.map(([k,v,cls])=>`<div class="stat"><span class="mut">${k}</span><b class="${cls||''}">${v}</b></div>`).join('');}

async function loadOverview(){const s=await api('/admin/stats');const t=s.totals,b=s.breakdown||{};
  $('#stats').innerHTML=statCards([['激活码总数',t.codesTotal],['在用码',t.codesActive,'ok'],['绑定设备',t.devicesTotal],['商店购买',t.storePurchases],['黑名单',t.blacklist,'bad']]);
  $('#stock').innerHTML=statCards([['未发出',b.codeUnused,'mut'],['使用中',b.codeInUse,'ok'],['已停用',b.codeRevoked,'bad']]);
  $('#expiry').innerHTML=statCards([['已到期',b.subExpired,'bad'],['7 天内',b.subExp7d,'warn'],['8–30 天',b.subExp30d,'warn'],['30 天后',b.subActive,'ok']]);
  $('#channels').innerHTML=statCards([['码渠道设备',t.devicesTotal],['商店渠道设备',b.storeDevices],['有效商店购买',b.storeActive,'ok']]);
  const days={};(s.activationsDaily||[]).forEach(d=>days[d.day]=d.n);
  let html='';const now=Math.floor(Date.now()/1000);const max=Math.max(1,...Object.values(days));
  for(let i=13;i>=0;i--){const d=Math.floor((now-i*86400)/86400)*86400;const n=days[d]||0;const h=Math.round(n/max*100);
    html+=`<div class="bar" style="height:${h}%" data-tip="${new Date(d*1000).toLocaleDateString('zh-CN')}: ${n}"></div>`;}
  $('#chart').innerHTML=html;}

function kindText(c){if(c.kind==='lifetime')return'买断';
  let txt='订阅'+c.durationDays+'天';
  if(c.anchor){const exp=c.anchor+c.durationDays*86400;const cls=exp*1000<Date.now()?'bad':(exp-Date.now()/1000<7*86400?'warn':'ok');txt+=`<br><span class="mut">到期 </span><span class="${cls}">${ts(exp)}</span>`;}
  return txt;}

async function loadCodes(){
  const p=new URLSearchParams();const q=$('#fq').value.trim();if(q)p.set('q',q);const st=$('#fs').value;if(st)p.set('status',st);
  lastCodes=await api('/admin/codes?'+p.toString());
  $('#codes').innerHTML=lastCodes.map(c=>`<tr><td style="font-family:monospace">${c.code}</td>
    <td>${kindText(c)}</td>
    <td class="${c.status==='active'?'ok':'bad'}">${c.status==='active'?'在用':'已停用'}</td>
    <td>${c.devices}/${c.maxDevices}</td>
    <td class="mut">${c.email||'-'}</td>
    <td class="mut">${ts(c.createdAt)}</td><td class="mut">${c.note||''}</td>
    <td><details ontoggle="if(this.open)devices('${c.code}')"><summary>管理</summary>
      <div class="editbox">
        <div class="row">设备上限 <input id="m-${c.code}" type="number" value="${c.maxDevices}" style="width:70px">
          <button class="mini primary" onclick="saveEdit('${c.code}')">保存</button></div>
        ${c.kind==='subscription'?`<div class="row">总天数 <input id="d-${c.code}" type="number" value="${c.durationDays||365}" style="width:90px">
          延期 <input id="e-${c.code}" type="number" placeholder="+天" style="width:80px">
          <button class="mini primary" onclick="extendCode('${c.code}')">延期/改期</button></div>`:''}
        <div class="row">邮箱 <input id="em-${c.code}" value="${c.email||''}" placeholder="改绑/清空=解绑" style="width:170px"></div>
        <div class="row">备注 <input id="n-${c.code}" value="${(c.note||'').replace(/"/g,'&quot;')}" style="width:150px">
          ${c.status==='active'
            ?`<button class="mini danger" onclick="setStatus('${c.code}','revoked')">停用</button>`
            :`<button class="mini ok" onclick="setStatus('${c.code}','active')">恢复</button>`}
          <button class="mini danger" onclick="delCode('${c.code}')">删除</button></div>
      </div>
      <div id="dev-${c.code}" style="margin-top:6px"></div></details></td></tr>`).join('')||'<tr><td colspan="8" class="mut">暂无</td></tr>';}

async function devices(code){const list=await api(`/admin/codes/${code}/devices`);
  const el=document.getElementById('dev-'+code);
  el.innerHTML='<table>'+list.map(d=>`<tr><td style="font-family:monospace;font-size:11px">${d.deviceId}</td>
    <td>${d.deviceName||'-'}</td><td>${d.platform||'-'}</td><td class="mut">${ts(d.lastSeen)}</td>
    <td><button class="mini danger" onclick="unbind('${code}','${d.deviceId}')">远程注销</button>
    <button class="mini danger" onclick="banDev('${d.deviceId}')">拉黑</button></td></tr>`).join('')+'</table>';}

async function issue(){const body={kind:$('#ck').value,maxDevices:+$('#cm').value||3,count:Math.min(+$('#cn').value||1,100),note:$('#cnote').value||null,email:$('#cemail').value.trim()||null};
  if(body.email&&!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(body.email))return toast('邮箱格式无效',false);
  if(!body.count)return toast('数量无效',false);
  if(body.kind==='subscription'){if(!+$('#cd').value)return toast('订阅码需填写天数',false);body.durationDays=+$('#cd').value;}
  try{const r=await api('/admin/codes',{method:'POST',body});lastIssued=r.codes;
    $('#issued').style.display='';$('#issued-title').textContent=`已生成 ${r.codes.length} 个：`;
    $('#issued-list').textContent=r.codes.join('\n');toast('生成成功');loadCodes();}catch(e){toast(e.message,false)}}
function issuedRows(){return(lastIssued||[]).map(c=>[c,$('#ck').value,$('#ck').value==='subscription'?$('#cd').value:'',$('#cm').value,'active',$('#cnote').value||'',$('#cemail').value.trim()||'',Math.floor(Date.now()/1000)]);}
function toCsv(rows){return['code,kind,durationDays,maxDevices,status,note,email,createdAt'].join(',')+'\n'+rows.map(r=>r.map(csvCell).join(',')).join('\n');}
function copyIssued(){navigator.clipboard.writeText(lastIssued.join('\n')).then(()=>toast('已复制'),()=>toast('复制失败',false));}
function downloadIssued(fmt){if(!lastIssued)return;download(`prism-codes-${Date.now()}.${fmt}`,fmt==='csv'?toCsv(issuedRows()):lastIssued.join('\n'));}
function exportFiltered(fmt){if(!lastCodes.length)return toast('没有可导出的数据',false);
  const rows=lastCodes.map(c=>[c.code,c.kind,c.durationDays||'',c.maxDevices,c.status,c.note||'',c.email||'',c.createdAt]);
  download(`prism-codes.${fmt}`,fmt==='csv'?toCsv(rows):lastCodes.map(c=>c.code).join('\n'));}

async function patchCode(code,body){try{await api(`/admin/codes/${code}`,{method:'PATCH',body});toast('已保存');loadCodes();}catch(e){toast(e.message,false)}}
function saveEdit(c){const body={maxDevices:+document.getElementById('m-'+c).value||undefined,note:document.getElementById('n-'+c).value||null,email:document.getElementById('em-'+c).value.trim()};if(!body.maxDevices)return toast('设备上限无效',false);patchCode(c,body);}
async function extendCode(c){
  const total=+document.getElementById('d-'+c).value;const add=+document.getElementById('e-'+c).value;
  const body={};if(add>0)body.extendDays=add;else if(total>0)body.durationDays=total;else return toast('填写总天数或延期天数',false);
  patchCode(c,body);}
function setStatus(c,s){if(!confirm(s==='revoked'?('停用 '+c+'？该码所有设备立即失效'):('恢复 '+c+'？')))return;patchCode(c,{status:s});}
async function delCode(c){if(!confirm('彻底删除 '+c+' 及其设备绑定？'))return;try{await api(`/admin/codes/${c}`,{method:'DELETE'});toast('已删除');loadCodes();}catch(e){toast(e.message,false)}}
async function unbind(c,d){if(!confirm('远程注销该设备？其下次校验将被拒绝，需重新激活'))return;
  try{await api(`/admin/codes/${c}/devices/${encodeURIComponent(d)}`,{method:'DELETE'});toast('已注销');devices(c);loadCodes();}catch(e){toast(e.message,false)}}

async function loadBlacklist(){const list=await api('/admin/blacklist');
  $('#bl').innerHTML=list.map(b=>`<tr><td style="font-family:monospace;font-size:11px">${b.deviceId}</td><td>${b.reason||'-'}</td>
    <td class="mut">${ts(b.createdAt)}</td><td><button class="mini" onclick="unban('${b.deviceId}')">解禁</button></td></tr>`).join('')||'<tr><td colspan="4" class="mut">暂无</td></tr>';}
async function ban(){const d=$('#bd').value.trim();if(!d)return toast('设备 ID 不能为空',false);
  try{await api('/admin/blacklist',{method:'POST',body:{deviceId:d,reason:$('#br').value||null}});$('#bd').value='';$('#br').value='';toast('已拉黑');loadBlacklist();}catch(e){toast(e.message,false)}}
async function banDev(d){const r=prompt('拉黑原因(可选)');if(r===null)return;
  try{await api('/admin/blacklist',{method:'POST',body:{deviceId:d,reason:r||null}});toast('已拉黑');loadBlacklist();}catch(e){toast(e.message,false)}}
async function unban(d){try{await api(`/admin/blacklist/${encodeURIComponent(d)}`,{method:'DELETE'});toast('已解禁');loadBlacklist();}catch(e){toast(e.message,false)}}

if(KEY){$('#key').value=KEY;boot();}
</script>
</body>
</html>"##;
