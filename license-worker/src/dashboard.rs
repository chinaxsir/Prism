//! 内嵌管理后台页面（GET /admin）。纯静态 HTML+原生 JS，
//! 管理密钥仅存浏览器 localStorage，所有请求携带 X-Admin-Key。

pub const PAGE: &str = r##"<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Prism 授权管理后台</title>
<style>
:root{--bg:#0f1220;--card:#1a1f36;--hover:#232a4a;--fg:#e6e9f5;--mut:#8b93b8;--acc:#5b7fff;--bad:#ff5b6a;--ok:#3ecf8e}
*{box-sizing:border-box;margin:0;padding:0}
body{background:var(--bg);color:var(--fg);font:14px/1.6 -apple-system,"PingFang SC","Microsoft YaHei",sans-serif;padding:24px;max-width:1100px;margin:0 auto}
h1{font-size:20px;margin-bottom:16px}
.card{background:var(--card);border-radius:12px;padding:16px;margin-bottom:16px}
.row{display:flex;gap:8px;flex-wrap:wrap;align-items:center}
input,select,button{background:var(--hover);border:1px solid #ffffff14;color:var(--fg);border-radius:8px;padding:8px 12px;font-size:13px}
button{cursor:pointer}button:hover{border-color:var(--acc)}
button.primary{background:var(--acc);border-color:var(--acc)}
button.danger{color:var(--bad)}
table{width:100%;border-collapse:collapse;font-size:12px}
th,td{text-align:left;padding:8px 6px;border-bottom:1px solid #ffffff0d;vertical-align:top}
th{color:var(--mut);font-weight:500}
.mut{color:var(--mut)}.ok{color:var(--ok)}.bad{color:var(--bad)}
.tag{display:inline-block;padding:1px 8px;border-radius:6px;background:var(--hover);font-size:11px;margin-right:4px}
.stats{display:grid;grid-template-columns:repeat(auto-fit,minmax(140px,1fr));gap:10px}
.stat{background:var(--hover);border-radius:10px;padding:14px}
.stat b{display:block;font-size:24px}
.chart{display:flex;align-items:flex-end;gap:4px;height:100px;margin-top:10px}
.bar{flex:1;background:var(--acc);border-radius:3px 3px 0 0;min-height:2px;position:relative}
.bar:hover::after{content:attr(data-tip);position:absolute;bottom:100%;left:50%;transform:translateX(-50%);background:#000;color:#fff;font-size:11px;padding:2px 6px;border-radius:4px;white-space:nowrap}
.tabs{display:flex;gap:4px;margin-bottom:16px}
.tabs button.on{background:var(--acc)}
#toast{position:fixed;top:16px;right:16px;background:var(--card);border:1px solid var(--acc);border-radius:8px;padding:10px 16px;display:none;z-index:9}
details{margin-top:6px}summary{cursor:pointer;color:var(--acc);font-size:12px}
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
    <div class="card"><div class="stats" id="stats"></div></div>
    <div class="card"><b>近 14 天激活</b><div class="chart" id="chart"></div></div>
  </div>

  <div id="tab-codes" style="display:none">
    <div class="card">
      <div class="row">
        <select id="ck"><option value="lifetime">买断</option><option value="subscription">订阅</option></select>
        <input id="cd" type="number" placeholder="天数(订阅)" style="width:100px">
        <input id="cm" type="number" placeholder="设备数" value="3" style="width:80px">
        <input id="cn" type="number" placeholder="数量" value="1" style="width:70px">
        <input id="cnote" placeholder="备注" style="width:140px">
        <button class="primary" onclick="issue()">生成</button>
        <span class="mut" id="newcodes"></span>
      </div>
    </div>
    <div class="card"><table><thead><tr><th>激活码</th><th>类型</th><th>状态</th><th>设备</th><th>创建</th><th>备注</th><th>操作</th></tr></thead><tbody id="codes"></tbody></table></div>
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
async function api(p,opt={}){opt.headers={'X-Admin-Key':KEY,'Content-Type':'application/json'};if(opt.body&&typeof opt.body!=='string')opt.body=JSON.stringify(opt.body);
  const r=await fetch(p,opt);if(r.status===401){logout();throw new Error('密钥错误');}
  const j=await r.json().catch(()=>({}));if(!r.ok)throw new Error(j.error&&j.error.message||('HTTP '+r.status));return j;}
function toast(m,ok){const t=$('#toast');t.textContent=m;t.style.borderColor=ok===false?'var(--bad)':'var(--acc)';t.style.display='block';setTimeout(()=>t.style.display='none',2500);}
function ts(s){return s?new Date(s*1000).toLocaleDateString('zh-CN'):'-'}
function saveKey(){KEY=$('#key').value.trim();localStorage.setItem('prism_admin_key',KEY);boot();}
function logout(){localStorage.removeItem('prism_admin_key');KEY='';$('#login').style.display='';$('#main').style.display='none';}
async function boot(){if(!KEY)return;try{await api('/admin/stats');}catch(e){return}$('#login').style.display='none';$('#main').style.display='';loadOverview();loadCodes();loadBlacklist();}
document.querySelectorAll('.tabs button[data-t]').forEach(b=>b.onclick=()=>{document.querySelectorAll('.tabs button[data-t]').forEach(x=>x.classList.remove('on'));b.classList.add('on');['overview','codes','blacklist'].forEach(t=>$('#tab-'+t).style.display=t===b.dataset.t?'':'none');});

async function loadOverview(){const s=await api('/admin/stats');const t=s.totals;
  $('#stats').innerHTML=[['激活码',t.codesTotal],['在用码',t.codesActive],['绑定设备',t.devicesTotal],['商店购买',t.storePurchases],['黑名单',t.blacklist]].map(([k,v])=>`<div class="stat"><span class="mut">${k}</span><b>${v}</b></div>`).join('');
  const days={};(s.activationsDaily||[]).forEach(d=>days[d.day]=d.n);
  let html='';const now=Math.floor(Date.now()/1000);const max=Math.max(1,...Object.values(days));
  for(let i=13;i>=0;i--){const d=Math.floor((now-i*86400)/86400)*86400;const n=days[d]||0;const h=Math.round(n/max*100);
    html+=`<div class="bar" style="height:${h}%" data-tip="${new Date(d*1000).toLocaleDateString('zh-CN')}: ${n}"></div>`;}
  $('#chart').innerHTML=html;}

async function loadCodes(){const list=await api('/admin/codes');
  $('#codes').innerHTML=list.map(c=>`<tr><td style="font-family:monospace">${c.code}</td>
    <td>${c.kind==='lifetime'?'买断':'订阅'+(c.durationDays?c.durationDays+'天':'')}</td>
    <td class="${c.status==='active'?'ok':'bad'}">${c.status}</td><td>${c.devices}/${c.maxDevices}</td>
    <td class="mut">${ts(c.createdAt)}</td><td class="mut">${c.note||''}</td>
    <td><details><summary>管理</summary>
      <div class="row" style="margin-top:6px">
      <button onclick="devices('${c.code}')">设备</button>
      ${c.status==='active'?`<button onclick="revoke('${c.code}')">吊销</button>`:''}
      <button class="danger" onclick="delCode('${c.code}')">删除</button></div>
      <div id="dev-${c.code}"></div></details></td></tr>`).join('')||'<tr><td colspan="7" class="mut">暂无</td></tr>';}

async function devices(code){const list=await api(`/admin/codes/${code}/devices`);
  $(`#dev-${CSS.escape(code)}`).innerHTML='<table>'+list.map(d=>`<tr><td style="font-family:monospace;font-size:11px">${d.deviceId}</td>
    <td>${d.deviceName||'-'}</td><td>${d.platform||'-'}</td><td class="mut">${ts(d.lastSeen)}</td>
    <td><button onclick="unbind('${code}','${d.deviceId}')">解绑</button>
    <button class="danger" onclick="banDev('${d.deviceId}')">拉黑</button></td></tr>`).join('')+'</table>';}

async function issue(){const body={kind:$('#ck').value,maxDevices:+$('#cm').value||3,count:+$('#cn').value||1,note:$('#cnote').value||null};
  if(body.kind==='subscription'&&+$('#cd').value>0)body.durationDays=+$('#cd').value;
  try{const r=await api('/admin/codes',{method:'POST',body});$('#newcodes').textContent='已生成: '+r.codes.join(', ');loadCodes();}catch(e){toast(e.message,false)}}
async function revoke(c){if(!confirm('吊销 '+c+'？'))return;try{await api(`/admin/codes/${c}/revoke`,{method:'POST'});toast('已吊销');loadCodes();}catch(e){toast(e.message,false)}}
async function delCode(c){if(!confirm('彻底删除 '+c+' 及其设备绑定？'))return;try{await api(`/admin/codes/${c}`,{method:'DELETE'});toast('已删除');loadCodes();}catch(e){toast(e.message,false)}}
async function unbind(c,d){try{await api(`/admin/codes/${c}/devices/${encodeURIComponent(d)}`,{method:'DELETE'});toast('已解绑');devices(c);}catch(e){toast(e.message,false)}}

async function loadBlacklist(){const list=await api('/admin/blacklist');
  $('#bl').innerHTML=list.map(b=>`<tr><td style="font-family:monospace;font-size:11px">${b.deviceId}</td><td>${b.reason||'-'}</td>
    <td class="mut">${ts(b.createdAt)}</td><td><button class="danger" onclick="unban('${b.deviceId}')">解禁</button></td></tr>`).join('')||'<tr><td colspan="4" class="mut">暂无</td></tr>';}
async function ban(){const d=$('#bd').value.trim();if(!d)return toast('设备 ID 不能为空',false);
  try{await api('/admin/blacklist',{method:'POST',body:{deviceId:d,reason:$('#br').value||null}});$('#bd').value='';$('#br').value='';toast('已拉黑');loadBlacklist();}catch(e){toast(e.message,false)}}
async function banDev(d){const r=prompt('拉黑原因(可选)');if(r===null)return;
  try{await api('/admin/blacklist',{method:'POST',body:{deviceId:d,reason:r||null}});toast('已拉黑');loadBlacklist();}catch(e){toast(e.message,false)}}
async function unban(d){try{await api(`/admin/blacklist/${encodeURIComponent(d)}`,{method:'DELETE'});toast('已解禁');loadBlacklist();}catch(e){toast(e.message,false)}}

if(KEY){$('#key').value=KEY;boot();}
</script>
</body>
</html>"##;
