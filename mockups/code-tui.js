// The whole product preview is a terminal cell buffer. The webpage is just a viewer.
const W=112,H=36,CW=9,CH=20,FS=14;
const concepts=[
 {id:'ribbon',name:'Ribbon',tag:'Conversation-first. No sidebars.',description:'An open transcript with indented tool receipts, a live inline patch, and a low-profile command ribbon. The only strong color belongs to the next action.',bg:'#101116',fg:'#dedfe7',muted:'#9497a8',dim:'#555a70',line:'#34384a',panel:'#1b1e29',accent:'#b7adfa',green:'#a0c9b2',red:'#df9da8',add:'#192b27',del:'#30212a'},
 {id:'mosaic',name:'Mosaic',tag:'A tiled terminal workspace.',description:'Four independently framed regions: conversation, live patch, run plan, and session health. Dense but organized, inspired by modern terminal multiplexers—not desktop cards.',bg:'#11181a',fg:'#e1e9e6',muted:'#99ada8',dim:'#5e7771',line:'#36514c',panel:'#192623',accent:'#a3d6c2',green:'#b3d6a0',red:'#dfa5a0',add:'#20362b',del:'#342828'},
 {id:'split',name:'Split',tag:'Source on the left. Agent on the right.',description:'A bright, code-first terminal: the patch occupies two thirds of the screen, with a compact agent channel alongside it. Real line numbers, test output, and pane-local prompts.',bg:'#e9edf2',fg:'#283d55',muted:'#62778f',dim:'#91a0b2',line:'#bfcbda',panel:'#d9e2ee',accent:'#315f9e',green:'#456d51',red:'#aa5960',add:'#d4e5d7',del:'#ecd8db'},
 {id:'trace',name:'Trace',tag:'A live execution ledger.',description:'A full-width, timestamped timeline. Read, edit, and test events have distinct gutters; the active command expands inline. No chat bubbles, tool tables, or permanent panels.',bg:'#101615',fg:'#d9e5de',muted:'#8ca99b',dim:'#49685b',line:'#2d483b',panel:'#192b23',accent:'#9dd4aa',green:'#a5dcae',red:'#e2a79c',add:'#1c3525',del:'#34251f'},
 {id:'still',name:'Still',tag:'A quiet terminal, not a dashboard.',description:'A single centered reading column. No frames, tabs, sidebar, timestamps, or status clutter. Tool activity collapses to one line; only the current thought and input remain prominent.',bg:'#141416',fg:'#e5e3de',muted:'#a29f98',dim:'#68665f',line:'#363530',panel:'#20201f',accent:'#d2d0c2',green:'#a9b9a0',red:'#c7a39c',add:'#232b24',del:'#302523'},
];
const scenes=['Welcome','Working','Review'];
const canvas=document.getElementById('terminal'),ctx=canvas.getContext('2d');
let direction=0,scene=1,paused=matchMedia('(prefers-reduced-motion: reduce)').matches,clock=0,P,cells,decision='',lastFrame=0,drawRatio=1;
const LOGO=['█ █ █ █ █▀▄ █▀▀ █▀▀ █▀▄','▀▄▀▄▀ █ █ █ █▀  ██▄ █▀▄'];
function reset(){P=concepts[direction];cells=Array.from({length:H},()=>Array.from({length:W},()=>({ch:' ',fg:P.fg,bg:P.bg,bold:false})));}
function text(x,y,s,color=P.fg,bold=false,bg){for(const ch of s){if(x>=0&&x<W&&y>=0&&y<H){const c=cells[y][x];c.ch=ch;c.fg=color;c.bold=bold;if(bg)c.bg=bg;}x++;}}
function fill(x,y,w,h,bg){for(let j=Math.max(0,y);j<Math.min(H,y+h);j++)for(let i=Math.max(0,x);i<Math.min(W,x+w);i++)cells[j][i]={ch:' ',fg:P.fg,bg,bold:false};}
function rule(x,y,w,color=P.line,ch='─'){text(x,y,ch.repeat(w),color);}
function vertical(x,y,h,color=P.line,ch='│'){for(let j=0;j<h;j++)text(x,y+j,ch,color);}
function frame(x,y,w,h,title='',active=false){const c=active?P.accent:P.line;rule(x+1,y,w-2,c);rule(x+1,y+h-1,w-2,c);vertical(x,y+1,h-2,c);vertical(x+w-1,y+1,h-2,c);text(x,y,'╭',c);text(x+w-1,y,'╮',c);text(x,y+h-1,'╰',c);text(x+w-1,y+h-1,'╯',c);if(title)text(x+2,y,' '+title+' ',active?P.fg:P.muted,true);}
function centered(y,s,color=P.fg,bold=false,x=0,w=W){text(x+Math.floor((w-[...s].length)/2),y,s,color,bold);}
function runMark(){return ['◐','◓','◑','◒'][Math.floor(clock*2)%4];}
function segments(x,y,segs){for(const [s,c,b]of segs){text(x,y,s,c,b);x+=[...s].length;}}
function badge(x,y,s,fg=P.accent,bg=P.panel){text(x,y,' '+s+' ',fg,true,bg);}
function bottom(label=''){text(3,35,'● Claude Code',P.accent);text(18,35,'opus · high · ask',P.muted);text(60,35,label||'23% ctx  ▰▰▱▱▱   $0.42   winfer ⎇ main',P.dim);}
function compactInput(y=31,x=3,w=106){rule(x,y,w);text(x,y+1,'❯',P.accent,true);text(x+3,y+1,scene===0?'What would you like to build?':'Ask a follow-up, or steer the agent…',P.muted);text(x,y+3,'/ commands   @ context   ! shell',P.dim);text(x+w-35,y+3,'ctrl+t model   ⇧tab mode   ↵ send',P.dim);}
function patch(x,y,w=68,short=false){fill(x,y,w,1,P.panel);text(x+1,y,'src/auth.ts',P.fg,true);text(x+w-12,y,'+24  −6',P.muted);const lines=short?[
 [' 42   export async function refresh(token) {',P.muted],
 [' 43 −   return sessions.find(token);',P.red,P.del],
 [' 43 +   const s = await consumeOnce(token);',P.green,P.add],
 [' 44 +   if (!s) throw new TokenReuseError();',P.green,P.add],
 [' 45 +   return rotateTokenFamily(s);',P.green,P.add],
 [' 46   }',P.muted],
 ]:[
 [' 37   // A refresh token is valid exactly once.',P.dim],
 [' 38   import { consumeOnce } from "./tokens";',P.muted],
 [' 39',P.dim],
 [' 40   export async function refresh(token) {',P.fg],
 [' 41 −   return sessions.find(token);',P.red,P.del],
 [' 41 +   const s = await consumeOnce(token);',P.green,P.add],
 [' 42 +   if (!s) {',P.green,P.add],
 [' 43 +     throw new TokenReuseError();',P.green,P.add],
 [' 44 +   }',P.green,P.add],
 [' 45 +   return rotateTokenFamily(s);',P.green,P.add],
 [' 46   }',P.fg],
 ];lines.forEach(([s,c,bg],i)=>{if(bg)fill(x,y+i+2,w,1,bg);text(x,y+i+2,s,c);});}
function approval(x,y,w=68){if(decision){badge(x,y,'✓ '+decision,P.green);text(x,y+2,'Preview only · no files were changed.',P.dim);return;}text(x,y,'Apply these changes?',P.fg,true);badge(x,y+2,'Y  approve',P.bg,P.accent);text(x+17,y+2,'A  always allow',P.muted);text(x+w-10,y+2,'N  deny',P.muted);}
function welcome(){const night=direction===2;const bg=night?'#101b2a':P.bg;fill(0,0,W,H,bg);const star=night?'#627c9e':P.dim;
 for(let i=0;i<96;i++){const x=(i*37+13)%W,y=(i*17+7)%29;if(y>=10&&y<=25)continue;const sparkle=Math.sin(clock*.8+i*1.7)>.7; text(x,y,['·','˙','✧','·','✦'][i%5],sparkle?(night?'#a9bce0':P.muted):star);}
 const palette=['#8aaee3','#aaa3eb','#c49fe2','#d6a2d0','#e3a8be'];LOGO.forEach((line,j)=>[...line].forEach((ch,i)=>text(44+i,10+j,ch,palette[Math.min(4,Math.floor(i/5))],true)));
 const muted=night?'#829ab8':P.muted,fg=night?'#d1dcf0':P.fg;
 centered(14,'winfer code',fg,true);centered(15,'a little space for your next big idea',muted);
 centered(19,'● Claude Code   opus · high · ask',fg);centered(21,'~/Coding/winfer   ⎇ main',muted);
 const keys=[['/','commands'],['@','context'],['!','shell'],['ctrl+t','model'],['⇧tab','mode']];
 if(direction===1){keys.forEach(([k,l],i)=>{const x=7+i*20;frame(x,24,18,3);text(x+2,25,k,P.accent,true);text(x+3+k.length,25,l,muted);});}
 else centered(25,'/ commands     @ context     ! shell     ctrl+t model     ⇧tab mode',muted);
 if(direction===2){fill(0,30,W,6,P.bg);text(3,31,'winfer ⎇ main',P.dim);compactInput(32,3,106);}
 else if(direction===3){fill(0,32,W,4,P.panel);badge(2,33,'READY',P.bg,P.accent);text(13,33,'❯ What would you like to build?',P.fg);text(80,33,'opus · high · ask',P.dim);}
 else if(direction===4){text(22,30,'›',P.fg);text(25,30,'What would you like to build?',P.muted);centered(35,'opus · high · ask',P.dim);}
 else{compactInput(30);bottom();}
}
function ribbon(){
 text(3,1,'✦ winfer code',P.accent,true);text(23,1,'winfer / refresh-token rotation',P.muted);text(83,1,'⎇ main   opus · high',P.dim);
 rule(3,3,106);segments(5,3,[[' ✓ inspect ',P.dim],['──',P.line],[' ✓ implement ',P.dim],['──',P.line],[` ${scene===2?'◇ review':runMark()+' verify'} `,P.accent,true],['──',P.line],[' ○ hand off ',P.dim]]);
 fill(7,5,102,3,P.panel);vertical(7,5,3,P.accent,'▏');text(10,5,'you',P.muted,true);text(10,6,'Add refresh-token rotation and cover the edge cases.',P.fg);
 text(3,10,'✦',P.accent);text(7,10,'winfer',P.fg,true);text(15,10,'opus',P.dim);
 text(7,12,'Refresh tokens now rotate on every use. Reusing an old token revokes');text(7,13,'the whole family, so a stolen token cannot silently keep a session alive.');
 if(scene===1){text(7,16,'├─',P.line);text(11,16,'read',P.muted);text(20,16,'src/auth.ts',P.fg);text(49,16,'146 lines',P.dim);text(7,17,'├─',P.line);text(11,17,'edit',P.muted);text(20,17,'src/auth.ts',P.fg);text(49,17,'+24 −6',P.green);text(7,18,'╰─',P.line);text(11,18,'test',P.muted);text(20,18,'auth.test.ts',P.fg);text(49,18,'8 passed',P.green);
  fill(7,21,102,6,P.panel);text(10,21,'LIVE PATCH',P.dim);text(27,21,'src/auth.ts',P.muted);text(10,23,'+ const s = await consumeOnce(token);',P.green);text(10,24,'+ if (!s) throw new TokenReuseError();',P.green);text(10,25,'+ return rotateTokenFamily(s);',P.green);text(75,24,'+24 −6',P.green);text(75,25,'2 files changed',P.dim);
  text(7,29,runMark()+' Verifying token-reuse protection…',P.accent);text(79,29,'12s · esc to interrupt',P.dim);
 }else{patch(7,16,102,true);text(7,25,'✓ 8 tests passing · no new dependencies',P.green);approval(7,27,102);}
 compactInput();bottom();
}
function mosaic(){
 fill(0,0,W,1,P.panel);text(2,0,'✳ WINFER',P.accent,true);badge(16,0,'01 token rotation',P.bg,P.accent);text(39,0,'02 api client',P.dim);text(81,0,'opus / high / ask',P.muted);
 frame(1,2,75,14,'CONVERSATION');frame(77,2,34,20,'RUN PLAN',true);frame(1,17,75,12,scene===2?'PATCH REVIEW':'LIVE CHANGES');frame(77,23,34,6,'SESSION');
 text(4,4,'YOU',P.dim,true);text(4,6,'Add refresh-token rotation and cover the edge cases.');text(4,9,'✳ CLAUDE',P.accent,true);text(4,11,'Refresh tokens will be single-use. Reuse invalidates');text(4,12,'the whole token family; the session API stays the same.',P.muted);
 text(80,4,'01',P.dim);text(85,4,'✓ Inspect lifecycle',P.muted);text(85,6,'3 source files read',P.dim);text(80,9,'02',P.dim);text(85,9,'✓ Rotate tokens',P.muted);text(85,11,'2 files · +24 −6',P.dim);badge(80,14,'03',P.bg,P.accent);text(85,14,scene===2?'◇ Review patch':runMark()+' Verify cases',P.accent,true);text(85,16,'8 / 8 tests passing',P.muted);text(80,19,'04',P.dim);text(85,19,'○ Hand off',P.dim);
 patch(4,19,69,true);text(80,25,'CTX',P.dim);text(86,25,'▰▰▱▱▱ 23%',P.accent);text(80,27,'$0.42',P.muted);text(91,27,'12s elapsed',P.dim);
 if(scene===2){frame(1,30,110,4,'ALLOW EDIT?',true);if(decision)text(4,32,'✓ '+decision+' · preview only',P.green);else{badge(4,32,'Y approve',P.bg,P.accent);text(23,32,'A always allow',P.muted);text(93,32,'N deny',P.muted);}}
 else{frame(1,30,110,4,'COMMAND');text(4,32,'❯ Ask a follow-up…',P.muted);text(70,32,'/ commands   @ context   ↵ send',P.dim);}
 bottom('8/8 tests   2 changed   3 skills   ⎇ main');
}
function split(){
 fill(0,0,W,2,P.panel);text(2,0,'winfer',P.accent,true);text(13,0,'⎇ main',P.muted);text(85,0,'● opus · high · ask',P.accent);rule(0,2,W);
 fill(0,3,66,1,P.panel);text(2,3,'auth.ts  M',P.accent,true);text(19,3,'auth.test.ts  M',P.muted);text(42,3,'+24 −6',P.green);text(69,3,'AGENT / '+(scene===2?'REVIEW':'BUILD'),P.accent,true);vertical(66,3,31);
 text(3,5,'src › auth › auth.ts › refresh',P.dim);patch(2,7,63);text(3,22,'TEST RESULTS',P.accent,true);rule(2,23,63);text(4,25,'✓ rotates a valid refresh token',P.green);text(4,26,'✓ invalidates the previous token',P.green);text(4,27,'✓ revokes the family on reuse',P.green);text(4,29,'8 passed · 184ms · 0 warnings',P.muted);
 text(69,6,'YOU',P.dim,true);text(69,8,'Add refresh-token rotation and');text(69,9,'cover the edge cases.');text(69,12,'✦ CLAUDE',P.accent,true);text(69,14,'Refresh tokens now rotate on use.');text(69,15,'Token reuse revokes the family.',P.muted);text(69,18,'✓ inspected auth.ts',P.muted);text(69,20,'✓ patched 2 files',P.muted);text(69,22,'✓ all 8 tests pass',P.green);
 if(scene===2){text(69,25,'Apply the patch?',P.accent,true);if(decision)text(69,27,'✓ '+decision,P.green);else{badge(69,27,'Y approve',P.bg,P.accent);text(86,27,'N deny',P.muted);}}
 else{text(69,25,runMark()+' Final verification…',P.accent);}
 rule(69,30,40);text(69,31,'❯ Ask about this change…',P.muted);text(69,33,'@ context    ⇧tab mode    ↵ send',P.dim);fill(0,35,W,1,P.panel);text(2,35,'2 modified files',P.accent);text(25,35,'typescript',P.muted);text(79,35,'23% ctx   $0.42   main',P.muted);
}
function trace(){
 fill(0,0,W,2,P.panel);badge(2,0,'WINFER',P.bg,P.accent);text(13,0,'SESSION 01 / TOKEN ROTATION',P.accent,true);text(79,0,'● OPUS · HIGH · ASK',P.muted);text(2,3,'TIME',P.dim);text(14,3,'EVENT',P.dim);text(27,3,'EXECUTION TRACE',P.dim);rule(2,4,108);
 const events=[
  [6,'14:32:01','YOU','Add refresh-token rotation and cover the edge cases.',P.fg],
  [9,'14:32:03','PLAN','inspect  →  implement  →  verify  →  review',P.muted],
  [12,'14:32:05','READ','src/auth.ts',P.fg],
  [16,'14:32:08','EDIT','src/auth.ts',P.fg],
  [23,'14:32:12','TEST','pnpm test auth',P.fg],
 ];vertical(24,6,23,P.line,'│');events.forEach(([y,time,event,body,c])=>{text(2,y,time,P.dim);text(14,y,event,event==='YOU'?P.muted:P.accent,true);text(24,y,'●',P.accent);text(27,y,body,c);});
 text(27,7,'I’ll make refresh tokens single-use and revoke reused token families.',P.muted);text(27,10,'✓ lifecycle   ✓ rotation   '+(scene===2?'✓ tests   ◇ review':runMark()+' edge cases'),P.dim);text(27,13,'146 lines · 3 related files · 0.2s',P.dim);
 fill(27,17,81,4,P.add);text(29,17,'+ const s = await consumeOnce(token);',P.green);text(29,18,'+ if (!s) throw new TokenReuseError();',P.green);text(29,19,'+ return rotateTokenFamily(s);',P.green);text(88,19,'+24 −6',P.green);text(27,21,'2 files changed · no new dependencies',P.dim);text(27,24,'✓ 8 passed  /  0 failed  /  184ms',P.green);
 text(2,27,'14:32:14',P.dim);text(14,27,scene===2?'WAIT':'LIVE',P.accent,true);text(24,27,'◉',P.accent);text(27,27,scene===2?'Patch verified. Your approval is required.':runMark()+' Verifying token-reuse protection…',P.accent);
 if(scene===2){fill(0,31,W,3,P.panel);if(decision)text(3,32,'✓ '+decision+' · preview only',P.green);else{badge(3,32,'Y APPROVE',P.bg,P.accent);text(22,32,'A ALWAYS ALLOW',P.muted);text(95,32,'N DENY',P.muted);}}
 else{fill(0,31,W,3,P.panel);text(3,32,'❯',P.accent,true);text(6,32,'Send a correction or the next instruction…',P.muted);text(84,32,'↵ send   esc stop',P.dim);}
 text(2,35,'⎇ main   3 skills   2 MCP',P.dim);text(67,35,'12s elapsed   23% context   $0.42',P.muted);
}
function still(){
 centered(1,'winfer / opus',P.dim);text(20,5,'you',P.dim);text(20,7,'Add refresh-token rotation and cover the edge cases.');text(20,11,'✦',P.fg);text(24,11,'winfer',P.muted);
 if(scene===1){text(24,14,'A refresh token should work exactly once.');text(24,16,'I’ve added rotation on every use. If an old token is reused,');text(24,17,'the whole family is revoked—not just the individual token.',P.muted);text(24,20,'The session API stays the same. No new dependencies.',P.muted);text(24,24,'✓ read   ·   ✓ edited +24 −6   ·   ✓ 8 tests',P.dim);text(24,27,runMark()+' One final check on token reuse…',P.fg);}
 else{ text(24,14,'Ready when you are.',P.fg,true);patch(24,17,66,true);text(24,26,'✓ All 8 tests passing.',P.muted);if(decision)text(24,28,'✓ '+decision,P.green);else segments(24,28,[['y',P.fg,true],[' approve     ',P.dim],['a',P.fg,true],[' always     ',P.dim],['n',P.fg,true],[' deny',P.dim]]);}
 text(20,32,'›',P.fg,true);text(24,32,'Ask a follow-up…',P.dim);centered(35,'/ commands    @ context    ! shell    ctrl+t model',P.dim);
}
// Box-drawing and block glyphs are painted exactly on cell boundaries, as a terminal does.
const lines={'─':'lr','│':'ud','┌':'rd','┐':'ld','└':'ru','┘':'lu','├':'udr','┤':'udl','┬':'lrd','┴':'lru','┼':'udlr','╭':'rd','╮':'ld','╰':'ru','╯':'lu'};
function drawCell(c,x,y){const px=x*CW,py=y*CH;ctx.fillStyle=c.bg;ctx.fillRect(px,py,CW,CH);if(c.ch===' ')return;ctx.fillStyle=c.fg;if(lines[c.ch]){ctx.strokeStyle=c.fg;ctx.lineWidth=1;const mx=px+CW/2,my=py+CH/2;ctx.beginPath();for(const d of lines[c.ch]){ctx.moveTo(mx,my);ctx.lineTo(d==='l'?px:d==='r'?px+CW:mx,d==='u'?py:d==='d'?py+CH:my);}ctx.stroke();return;}if(c.ch==='▏'){ctx.fillRect(px,py,1,CH);return;}if(c.ch==='█'){ctx.fillRect(px,py,CW,CH);return;}if(c.ch==='▀'){ctx.fillRect(px,py,CW,CH/2);return;}if(c.ch==='▄'){ctx.fillRect(px,py+CH/2,CW,CH/2);return;}if(c.ch==='▰'||c.ch==='▱'){if(c.ch==='▰')ctx.fillRect(px+1,py+7,CW-2,6);else{ctx.strokeStyle=c.fg;ctx.lineWidth=1;ctx.strokeRect(px+1.5,py+7.5,CW-3,5);}return;}ctx.font=`${c.bold?'600':'400'} ${FS}px "JetBrains Mono",Menlo,monospace`;ctx.textAlign='center';ctx.textBaseline='alphabetic';ctx.fillText(c.ch,px+CW/2,py+15);}
function paint(){reset();if(scene===0)welcome();else [ribbon,mosaic,split,trace,still][direction]();ctx.setTransform(drawRatio,0,0,drawRatio,0,0);for(let y=0;y<H;y++)for(let x=0;x<W;x++)drawCell(cells[y][x],x,y);canvas.setAttribute('aria-label',`${P.name} — ${scenes[scene]} terminal mockup. ${P.tag}`);}
function resize(){const width=canvas.parentElement.clientWidth,dpr=window.devicePixelRatio||1;drawRatio=width*dpr/(W*CW);canvas.width=Math.ceil(W*CW*drawRatio);canvas.height=Math.ceil(H*CH*drawRatio);canvas.style.height=`${width*H*CH/(W*CW)}px`;paint();}
function sync(){document.querySelectorAll('#designs button').forEach((b,i)=>b.setAttribute('aria-pressed',i===direction));document.querySelectorAll('#screens button').forEach((b,i)=>b.setAttribute('aria-pressed',i===scene));const c=concepts[direction];document.getElementById('title').textContent=`0${direction+1} / ${c.name} — ${c.tag}`;document.getElementById('description').textContent=c.description;document.getElementById('motion').textContent=paused?'Resume motion':'Pause motion';history.replaceState(null,'',`#${c.id}/${scenes[scene].toLowerCase()}`);paint();}
function select(i){direction=i;decision='';sync();}
function setScene(i){scene=(i+3)%3;decision='';sync();}
concepts.forEach((c,i)=>{const b=document.createElement('button');b.innerHTML=`<small>${i+1}</small>${c.name}`;b.onclick=()=>select(i);document.getElementById('designs').append(b);});
scenes.forEach((s,i)=>{const b=document.createElement('button');b.textContent=s;b.onclick=()=>setScene(i);document.getElementById('screens').append(b);});
document.getElementById('motion').onclick=()=>{paused=!paused;sync();};
addEventListener('keydown',e=>{if(e.altKey||e.metaKey||e.ctrlKey||['INPUT','TEXTAREA'].includes(e.target.tagName))return;if(/^[1-5]$/.test(e.key)){e.preventDefault();select(Number(e.key)-1);}if(e.key==='ArrowLeft'||e.key==='ArrowRight'){e.preventDefault();setScene(scene+(e.key==='ArrowRight'?1:-1));}if(scene===2&&['y','n','a'].includes(e.key.toLowerCase())){e.preventDefault();decision={y:'Approved once',n:'Denied',a:'Always allowed'}[e.key.toLowerCase()];sync();const notice=document.getElementById('notice');notice.textContent=decision+' — mockup only; no files changed';notice.classList.add('visible');setTimeout(()=>notice.classList.remove('visible'),2200);}});
const [d,s]=location.hash.slice(1).split('/');const di=concepts.findIndex(c=>c.id===d),si=scenes.findIndex(c=>c.toLowerCase()===s);if(di>=0)direction=di;if(si>=0)scene=si;
document.fonts.load(`${FS}px "JetBrains Mono"`).catch(()=>{}).finally(()=>{resize();sync();});addEventListener('resize',resize);
function animate(now){if(now-lastFrame>150){if(!paused&&!document.hidden){clock=now/1000;paint();}lastFrame=now;}requestAnimationFrame(animate);}requestAnimationFrame(animate);
// Read-only diagnostics for verifying that every design is a bounded terminal buffer.
window.tuiDiagnostics=()=>({direction:concepts[direction].name,screen:scenes[scene],columns:W,rows:H,cells:cells.length*cells[0].length,decision,text:cells.map(row=>row.map(c=>c.ch).join('')).join('\n')});
