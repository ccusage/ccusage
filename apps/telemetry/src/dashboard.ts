export const dashboardHtml = String.raw`<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<meta name="description" content="Public ccusage activity statistics, aggregated daily without publishing installation identifiers.">
<title>ccusage · Public usage statistics</title>
<style>
:root{color-scheme:light dark;--bg:#f6f7fb;--panel:#fff;--text:#172139;--muted:#59657d;--border:#dfe4ef;--accent:#5145cd;--daily:#5145cd;--weekly:#09836e;--monthly:#c27513}*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--text);font:16px/1.6 system-ui,sans-serif}main{max-width:1120px;margin:auto;padding:40px 24px 64px}a{color:var(--accent);text-underline-offset:3px}nav{display:flex;justify-content:space-between;gap:24px;align-items:center;border-bottom:1px solid var(--border);padding-bottom:24px}.brand{font:700 24px ui-monospace,monospace;letter-spacing:-1px;text-decoration:none}nav span{color:var(--muted);font-size:14px}header{padding:44px 0 24px}h1{font-size:clamp(30px,5vw,44px);letter-spacing:-1.5px;line-height:1.2;margin:10px 0 16px}h2{font-size:19px;letter-spacing:-.4px;margin:0}p{margin:8px 0;color:var(--muted)}.eyebrow{font-size:12px;letter-spacing:2px;text-transform:uppercase;color:var(--accent);font-weight:700}.intro{max-width:780px}.cards{display:grid;grid-template-columns:repeat(3,1fr);gap:16px;margin:24px 0}.panel,.card{background:var(--panel);border:1px solid var(--border);border-radius:14px;padding:24px}.card h2{font-size:14px;color:var(--muted);font-weight:500}.number{font-size:clamp(28px,5vw,42px);font-weight:650;letter-spacing:-1.5px;line-height:1.5}.card small{color:var(--muted)}.toolbar{display:flex;justify-content:space-between;align-items:center;gap:16px;flex-wrap:wrap}.periods{display:flex;gap:6px}button{font:inherit;font-size:13px;border:1px solid var(--border);border-radius:7px;padding:6px 12px;background:var(--panel);color:var(--muted);cursor:pointer}button[aria-pressed=true]{background:var(--accent);color:#fff;border-color:var(--accent)}button:focus-visible,a:focus-visible,summary:focus-visible{outline:3px solid var(--accent);outline-offset:4px}.legend{display:flex;gap:20px;flex-wrap:wrap;font-size:13px;color:var(--muted);padding:16px 0}.legend span:before{content:'';display:inline-block;width:18px;height:3px;margin-right:7px;vertical-align:middle;background:var(--daily)}.legend span:nth-child(2):before{background:var(--weekly)}.legend span:nth-child(3):before{background:var(--monthly)}svg{display:block;width:100%;height:auto;overflow:visible}svg text{fill:var(--muted);font:12px system-ui,sans-serif}.split{display:grid;grid-template-columns:1fr 1fr;gap:16px;margin:16px 0}.breakdown{padding:14px 0 0;list-style:none;margin:0}.breakdown li{margin:14px 0}.row{display:flex;justify-content:space-between;gap:14px;font-size:14px;overflow-wrap:anywhere}.track{height:6px;border-radius:5px;background:var(--bg);margin-top:7px;overflow:hidden}.fill{height:100%;background:var(--accent);border-radius:5px}.meta{font-size:13px}.notice{border-left:3px solid var(--accent);padding:12px 18px;margin:20px 0;background:var(--panel)}.method{margin-top:30px;border-top:1px solid var(--border);padding-top:24px;max-width:860px}.method p{font-size:14px}details{margin-top:20px}summary{cursor:pointer;font-size:14px;color:var(--muted)}.table-scroll{overflow:auto;max-height:360px;margin-top:12px}table{border-collapse:collapse;width:100%;font-size:13px;text-align:right;font-variant-numeric:tabular-nums}th,td{padding:9px;border-bottom:1px solid var(--border)}th:first-child,td:first-child{text-align:left}footer{font-size:13px;color:var(--muted);margin-top:32px;display:flex;gap:20px;flex-wrap:wrap}[hidden]{display:none!important}@media(max-width:600px){main{padding:24px 16px}.cards{gap:8px}.card{padding:16px 10px}.card h2{font-size:12px}.card small{font-size:11px}.panel{padding:18px}.split{grid-template-columns:1fr}nav span{font-size:12px}header{padding-top:30px}}@media(prefers-color-scheme:dark){:root{--bg:#101522;--panel:#192133;--text:#edf0f8;--muted:#a8b3ca;--border:#303c54;--accent:#b0a5ff;--daily:#b0a5ff;--weekly:#63d5b5;--monthly:#efb456}button[aria-pressed=true]{color:#101522}}
</style><script src="/dashboard.js" defer></script></head>
<body><main><nav><a class="brand" href="https://ccusage.com">ccusage</a><span>Open source. Public statistics.</span></nav>
<header><div class="eyebrow">Usage, in the open</div><h1>A clearer picture of ccusage.</h1><p class="intro">Daily activity from participating ccusage installations. The same aggregate numbers we use to understand adoption are available to everyone.</p></header>
<div id="status" class="notice" role="status" aria-live="polite">Loading public statistics…</div>
<section id="statistics" hidden aria-label="Public activity statistics">
<p id="updated" class="meta"></p><div class="cards">
<article class="card"><h2>Daily active environments</h2><div class="number" id="daily">—</div><small>1 UTC day</small></article>
<article class="card"><h2>Weekly active environments</h2><div class="number" id="weekly">—</div><small>7 UTC days</small></article>
<article class="card"><h2>Monthly active environments</h2><div class="number" id="monthly">—</div><small>30 UTC days</small></article></div>
<section class="panel" aria-labelledby="activity-title"><div class="toolbar"><h2 id="activity-title">Activity over time</h2><div class="periods" aria-label="Chart period"><button type="button" data-days="30" aria-pressed="true">30 days</button><button type="button" data-days="90" aria-pressed="false">90 days</button></div></div>
<div class="legend"><span>Daily</span><span>Weekly</span><span>Monthly</span></div>
<svg id="chart" viewBox="0 0 960 300" role="img" aria-label="Daily, weekly and monthly active environments. Exact values are in the data table below."></svg>
<p class="meta">Missing daily aggregates are gaps, not zero activity. Rolling counts are distinct IDs within each window; daily totals cannot be added to calculate them.</p>
<details><summary>View activity data as a table</summary><div class="table-scroll"><table><thead><tr><th scope="col">UTC day</th><th scope="col">Daily</th><th scope="col">Weekly</th><th scope="col">Monthly</th></tr></thead><tbody id="activity-rows"></tbody></table></div></details></section>
<div class="split"><section class="panel"><h2>Platforms</h2><p class="meta">Latest available day</p><ul class="breakdown" id="platforms"></ul></section><section class="panel"><h2>Versions</h2><p class="meta">Latest available day</p><ul class="breakdown" id="versions"></ul></section></div></section>
<section class="method" aria-labelledby="method-title"><h2 id="method-title">What these numbers mean</h2>
<p>An active environment is a distinct random ID observed during a UTC window, not a verified person. Multiple devices, reset IDs, opt-outs, blocked requests and failed sends affect the counts. These are adoption indicators, not an exact population or a guaranteed lower bound.</p>
<p>The CLI sends at most one ping per UTC day with an installation ID, version, OS, architecture and command name. Nothing from usage logs, prompts or costs is sent. This page publishes aggregate counts only, never installation IDs. Command breakdowns are omitted because the first command of the day does not represent all command usage.</p>
<p>Aggregates are generated daily after 00:15 UTC. The public API is cached at the edge for up to one hour and in browsers for five minutes. No page analytics or third-party scripts are used.</p>
<p>To opt out, run <code>ccusage telemetry disable</code> or set <code>DO_NOT_TRACK=1</code>. <a href="https://ccusage.com/guide/telemetry">Read the collection and storage policy</a>.</p></section>
<footer><a href="/v1/stats">Download aggregate JSON</a><a href="https://github.com/ccusage/ccusage/tree/main/apps/telemetry">Inspect the source</a><a href="https://github.com/sponsors/ryoppippi">Support ccusage</a></footer></main></body></html>`;

export const dashboardScript = String.raw`'use strict';
(async () => {
 const status = document.getElementById('status');
 const number = new Intl.NumberFormat('en-GB');
 const DAY = 86400000;
 const node = (tag, text) => { const el = document.createElement(tag); if (text !== undefined) el.textContent = text; return el; };
 try {
  const response = await fetch('/v1/stats');
  if (!response.ok) throw new Error('Statistics unavailable');
  const data = await response.json();
  if (!Array.isArray(data.days)) throw new Error('Invalid statistics');
  const latest = data.days.at(-1);
  if (!latest) { status.textContent = 'No daily aggregates yet. Statistics will appear after the first successful daily rollup.'; return; }
  document.getElementById('statistics').hidden = false;
  const age = Date.now() - Date.parse(latest.date + 'T00:00:00Z');
  status.hidden = age < 2 * DAY;
  status.textContent = 'The latest aggregate is older than expected. Recent activity is not yet available.';
  document.getElementById('updated').textContent = 'Latest activity: ' + latest.date + ' UTC · Aggregated ' + data.generatedAt;
  for (const [id, key] of [['daily','dailyActiveUsers'],['weekly','weeklyActiveUsers'],['monthly','monthlyActiveUsers']]) document.getElementById(id).textContent = number.format(latest[key]);
  function breakdown(id, rows, label) {
   const list = document.getElementById(id);
   if (!rows.length) { list.append(node('li','No recorded activity.')); return; }
   const maximum = Math.max(1, ...rows.map(row => row.users));
   for (const row of rows) {
    const item = node('li'); const heading = node('div'); heading.className = 'row';
    heading.append(node('span',label(row)),node('span',number.format(row.users)));
    const track = node('div'); track.className = 'track'; track.setAttribute('aria-hidden','true');
    const fill = node('div'); fill.className = 'fill'; fill.style.width = (row.users / maximum * 100) + '%';
    track.append(fill); item.append(heading,track); list.append(item);
   }
  }
  breakdown('platforms',latest.byPlatform,row => row.os + ' / ' + row.arch);
  breakdown('versions',latest.byVersion,row => row.version);
  function chart(period) {
   const end = Date.parse(latest.date + 'T00:00:00Z'); const start = end - (period - 1) * DAY;
   const rows = data.days.filter(row => Date.parse(row.date + 'T00:00:00Z') >= start);
   const svg = document.getElementById('chart'); svg.replaceChildren();
   const svgNode = (tag, attrs, text) => { const el = document.createElementNS('http://www.w3.org/2000/svg',tag); for (const [key,value] of Object.entries(attrs)) el.setAttribute(key,String(value)); if (text !== undefined) el.textContent = text; svg.append(el); return el; };
   const max = Math.max(1,...rows.map(row => row.monthlyActiveUsers),...rows.map(row => row.weeklyActiveUsers),...rows.map(row => row.dailyActiveUsers));
   for (let i=0;i<=4;i++) { const y=24+i*58; svgNode('line',{x1:60,y1:y,x2:940,y2:y,stroke:'var(--border)'}); svgNode('text',{x:48,y:y+4,'text-anchor':'end'},number.format(Math.round(max*(1-i/4)))); }
   svgNode('text',{x:60,y:288},new Date(start).toISOString().slice(0,10)); svgNode('text',{x:940,y:288,'text-anchor':'end'},latest.date);
   for (const [key,color] of [['monthlyActiveUsers','monthly'],['weeklyActiveUsers','weekly'],['dailyActiveUsers','daily']]) {
    let path=''; let previous=null;
    for (const row of rows) { const time=Date.parse(row.date+'T00:00:00Z'); const x=60+(time-start)/(end-start)*880; const y=256-row[key]/max*232; path+=(previous === time-DAY ? ' L ' : ' M ')+x+' '+y; previous=time; svgNode('circle',{cx:x,cy:y,r:3,fill:'var(--'+color+')'}); }
    svgNode('path',{d:path,fill:'none',stroke:'var(--'+color+')','stroke-width':2.5,'stroke-linejoin':'round'});
   }
   const tbody=document.getElementById('activity-rows'); tbody.replaceChildren();
   for (const row of [...rows].reverse()) { const tr=node('tr'); const date=node('th',row.date); date.scope='row'; tr.append(date,...['dailyActiveUsers','weeklyActiveUsers','monthlyActiveUsers'].map(key => node('td',number.format(row[key])))); tbody.append(tr); }
  }
  for (const button of document.querySelectorAll('[data-days]')) button.addEventListener('click',() => { for (const other of document.querySelectorAll('[data-days]')) other.setAttribute('aria-pressed',String(other === button)); chart(Number(button.dataset.days)); });
  chart(30);
 } catch { status.hidden=false; status.textContent='Statistics could not be loaded. Please try again later, or check the aggregate JSON endpoint.'; }
})();`;
