// Structural proposals built from the current TUI's palette, vocabulary, and demo task.
// This is a terminal cell buffer, not HTML product UI.
const W=110,H=40;
const C={bg:'#0e0e13',fg:'#ececf2',muted:'#a9a9af',dim:'#7d7d83',line:'#434349',panel:'#1b1b20',blue:'#6fb6ff',violet:'#a98bff',pink:'#ff7ac6',green:'#5ee6c4',red:'#ff6b81',claude:'#ffa07a',add:'#19302c',del:'#301b27'};
const question='formatBytes prints NaN for files over 1 TB, can you fix it and add tests?';
let cells,scene,state;
function blank(){return Array.from({length:H},()=>Array.from({length:W},()=>({ch:' ',fg:C.fg,bg:C.bg,bold:false,italic:false,ul:''})));}
function put(x,y,s,fg=C.fg,bold=false,bg){for(const ch of s){if(cells[y]?.[x])cells[y][x]={...cells[y][x],ch,fg,bold,...(bg?{bg}:{})};x++;}}
function fill(x,y,w,h,bg){for(let j=y;j<y+h;j++)for(let i=x;i<x+w;i++)if(cells[j]?.[i])cells[j][i]={...cells[j][i],ch:' ',bg};}
function rule(x,y,w,color=C.line){put(x,y,'─'.repeat(w),color);}
function rail(x,y,h,color=C.line){for(let j=y;j<y+h;j++)put(x,j,'│',color);}
function box(x,y,w,h,title='',active=false){const color=active?C.violet:C.line;rule(x+1,y,w-2,color);rule(x+1,y+h-1,w-2,color);rail(x,y+1,h-2,color);rail(x+w-1,y+1,h-2,color);put(x,y,'╭',color);put(x+w-1,y,'╮',color);put(x,y+h-1,'╰',color);put(x+w-1,y+h-1,'╯',color);if(title)put(x+2,y,' '+title+' ',active?C.violet:C.muted,true);}
function label(x,y,s,fg=C.violet,bg=C.panel){put(x,y,' '+s+' ',fg,true,bg);}
function wrap(s,w){const out=[];let row='';for(const word of s.split(' ')){if(row.length+word.length+1>w&&row){out.push(row);row='';}row+=(row?' ':'')+word;}if(row)out.push(row);return out;}
function user(x,y,w=106){const lines=wrap(question,w-6);fill(x,y,w,lines.length+2,C.panel);for(let i=0;i<lines.length+2;i++)put(x,y+i,'▌',C.violet);put(x+2,y,'you',C.dim);lines.forEach((line,i)=>put(x+3,y+1+i,line,C.fg,true));}
function provider(x,y){put(x,y,'▐',C.claude);put(x+1,y,' Claude Code ',C.bg,true,C.claude);put(x+14,y,'▌',C.claude);}
function status(y=39){provider(2,y);put(19,y,'Opus 4.5 · high · ask',C.muted);put(65,y,'ctx ▰▰▱▱▱▱▱▱ 23%   winfer ⎇ main',C.dim);}
function title(extra=''){put(2,1,'✦ winfer code',C.violet,true);put(22,1,'winfer / fix formatBytes',C.dim);if(extra)put(81,1,extra,C.muted);}
function prompt(y=33,w=106,x=2){box(x,y,w,5,'',true);put(x+2,y+1,'❯',C.violet,true);put(x+4,y+1,scene==='conversation'?'ask anything…':'steer the agent… (tab queues it for after)',C.dim);put(x+2,y+3,'/ commands   @ files   ! shell',C.dim);put(x+w-32,y+3,'⇧tab mode   ctrl+t model   ↵',C.dim);}
function summary(x,y,w=100){const copy=scene==='conversation'?'Fixed. The unit loop now uses a unit table and template strings. TB values no longer print NaN.':'I’ll fix the unit loop, add TB test cases, and verify the existing API stays unchanged.';wrap(copy,w).forEach((l,i)=>put(x,y+i,l,C.fg));}
function read(x,y,w=100){put(x,y,'◆  ◇ read',C.blue);put(x+12,y,'src/lib/format.ts',C.fg,true);put(x+w-9,y,'0.1s  ▸',C.dim);const out=['export function formatBytes(n: number) {','  if (n < 1024) return n + " B";','  do { n /= 1024; } while (n > 1024);','}'];const n=state.expanded?4:2;rail(x,y+1,n);out.slice(0,n).forEach((l,i)=>put(x+3,y+i+1,l,C.dim));if(!state.expanded)put(x+w-9,y+2,'+4 lines',C.dim);}
function diff(x,y,w=100){fill(x,y,w,1,C.panel);put(x+1,y,'src/lib/format.ts',C.muted,true);put(x+w-10,y,'+3  −1',C.green);const lines=[[' 10   export function formatBytes(n: number) {',C.dim],[' 11 −   if (n < 1024) return n + " B";',C.red,C.del],[' 11 +   if (n < 1024) return `${n} B`;',C.green,C.add],[' 12 +   const units = ["KB", "MB", "GB"];',C.green,C.add],[' 13 +   let i = -1;',C.green,C.add],[' 14     do { n /= 1024; } while (n > 1024);',C.dim]];lines.forEach(([s,c,bg],i)=>{if(bg)fill(x,y+2+i,w,1,bg);put(x,y+2+i,s,c);});}
function decisions(x,y,w=100){if(state.decision){label(x,y,'✓ '+state.decision,C.green);put(x+31,y,'preview only · no files changed',C.dim);return;}label(x,y,'Y approve once',C.bg,C.violet);put(x+21,y,'A always allow',C.muted);put(x+w-9,y,'N deny',C.muted);}
function plan(x,y,w=100){put(x,y,'▸ plan',C.violet);put(x+9,y,'✓ inspect  ─  ✓ rewrite  ─  ◔ build & test  ─  ○ TB cases',C.muted);if(w>95)put(x+w-3,y,'2/4',C.dim);}
function agentRows(x,y,w=100){put(x,y,'◒ explore',C.violet);put(x+12,y,'Find other byte formatters',C.muted);put(x+w-5,y,'41s',C.dim);put(x,y+2,'◐ review',C.pink);put(x+12,y+2,'Audit date formatting',C.muted);put(x+w-5,y+2,'41s',C.dim);}
function working(x,y,w=100){put(x,y,scene==='conversation'?'✓ Turn complete  ·  1m 14s':'✺ Orbiting…  1m 14s',scene==='conversation'?C.green:C.violet,true);put(x+w-12,y,scene==='conversation'?'42 passed':'esc to stop',C.dim);}
function dock(){title('01 / conversation');user(2,4);put(4,9,'◌ thought for 4s',C.dim);put(2,12,'✦',C.violet);summary(6,12,99);
 if(scene==='approval'){diff(6,17,100);decisions(6,26,100);}else{read(6,17,100);put(6,24,scene==='conversation'?'✓ tests  42 passed':'◒  ❯ bash  cargo build --release',scene==='conversation'?C.green:C.pink);if(state.expanded)put(6,26,'Completed tool output is expanded. T folds it back.',C.dim);}
 box(2,28,106,10,'SESSION DOCK',true);plan(5,29,100);rule(4,30,102);working(5,31,100);put(5,33,'❯',C.violet,true);put(8,33,'steer the agent… (tab queues it for after)',C.dim);put(76,33,'@ files   / commands',C.dim);put(5,35,'↳ also update the changelog',C.muted);label(77,35,'queued · ↵ sends now',C.dim);provider(5,36);put(22,36,'Opus 4.5 · high · ask',C.muted);put(68,36,'23% ctx   2 agents   ⎇ main',C.dim);put(3,39,'T expand tool output',C.dim);put(78,39,'⇧tab mode  ctrl+t model',C.dim);}
function groups(){title('01 / turn groups');box(2,4,106,5,'YOU / TURN 01');put(5,6,question,C.fg,true);box(2,11,106,scene==='approval'?20:17,'✦ WINFER / '+(scene==='conversation'?'COMPLETED':'IN PROGRESS'));summary(6,13,97);
 if(scene==='approval'){diff(6,17,98);decisions(6,28,98);}else if(state.expanded){read(6,17,98);put(6,24,'◒  ❯ bash  cargo build --release',C.pink);}else{put(6,17,'✓ read',C.blue);put(22,17,'src/lib/format.ts',C.muted);put(91,17,'146 lines',C.dim);put(6,19,'✓ edit',C.violet);put(22,19,'src/lib/format.ts',C.muted);put(91,19,'+3 −1',C.green);put(6,21,scene==='conversation'?'✓ bash':'◒ bash',scene==='conversation'?C.green:C.pink);put(22,21,'cargo build --release',C.muted);put(91,21,scene==='conversation'?'done':'14s',C.dim);rule(5,23,100);put(6,25,'2 agents running',C.violet);put(30,25,'explore · byte formatters    review · date formatting',C.dim);}
 if(scene!=='approval'){working(5,29,100);put(5,31,'▸ plan  2/4   Build and run the test suite',C.muted);put(77,31,'T expand tools',C.dim);}prompt(33);status();}
function context(){const railOpen=state.rail;const w=railOpen?73:106;title(railOpen?'P hide context':'P show context');user(2,4,w);put(2,10,'✦',C.violet);summary(6,10,w-5);
 if(scene==='approval'){diff(5,15,w-6);decisions(5,26,w-6);}else{read(5,15,w-6);put(5,23,'◒  ❯ bash  cargo build --release',C.pink);put(8,25,'Compiling ratatui v0.30.2',C.dim);put(8,26,'Compiling winfer v0.1.73',C.dim);working(5,29,w-6);}
 if(railOpen){rail(78,3,28);put(82,4,'PLAN  /  2 OF 4',C.violet,true);put(82,7,'✓ Read format.ts',C.dim);put(82,9,'✓ Rewrite unit loop',C.dim);put(82,11,'◔ Build & test',C.fg,true);put(82,13,'○ Add TB test cases',C.dim);rule(82,15,24);put(82,17,'AGENTS / 2',C.violet);put(82,20,'◒ explore',C.muted);put(84,21,'byte formatters · 41s',C.dim);put(82,24,'◐ review',C.muted);put(84,25,'date formatting · 41s',C.dim);rule(82,27,24);put(82,29,'M format.ts    +3 −1',C.green);}
 put(5,31,'↳ queued: also update the changelog',C.dim);prompt(33);status();}
function reviewTray(){title('changes stay visible');user(2,4);put(2,10,'✦',C.violet);summary(6,10,100);put(6,14,'✓ read src/lib/format.ts   ·   '+(scene==='approval'?'edit awaiting permission':'edit +3 −1'),C.muted);const y=state.expanded?16:19;
 box(2,y,67,12,'CHANGE TRAY / 1 FILE',scene==='approval');diff(5,y+2,61);box(71,y,37,12,'VERIFICATION');put(74,y+2,scene==='conversation'?'✓ TESTS PASSED':'○ WAITING FOR BUILD',scene==='conversation'?C.green:C.dim);put(74,y+5,'unit tests      '+(scene==='conversation'?'42 passed':'queued'),C.muted);put(74,y+7,'TB edge cases   '+(scene==='conversation'?'3 passed':'pending'),C.muted);put(74,y+9,'changes         +3 −1',C.green);
 if(scene==='approval')decisions(5,32,100);else{working(5,32,100);}prompt(34);status();}
function drawer(){title('T switches activity');user(2,4);put(2,10,'✦',C.violet);summary(6,10,99);if(scene==='conversation'){put(6,15,'What changed',C.fg,true);put(6,17,'• format.ts — unit table and template strings',C.muted);put(6,19,'• format.test.ts — 3 new cases for TB values',C.muted);}else{put(6,16,'The unit loop is isolated. I’m checking compilation before adding',C.muted);put(6,17,'the regression cases for large sizes.',C.muted);}
 plan(3,21,104);box(2,23,106,9,'ACTIVITY DRAWER');const tab=scene==='approval'?1:state.tab;const labels=['Read','Changes','Build','Agents'];let x=5;labels.forEach((l,i)=>{label(x,24,l,tab===i?C.bg:C.dim,tab===i?C.violet:C.bg);x+=l.length+8;});rule(4,25,102);
 if(tab===0){put(6,27,'◇ read src/lib/format.ts',C.blue);put(6,29,'export function formatBytes(n: number) {',C.dim);put(56,29,'146 lines · 0.1s',C.dim);}if(tab===1){put(6,27,'+ if (n < 1024) return `${n} B`;',C.green);put(6,28,'+ const units = ["KB", "MB", "GB"];',C.green);put(6,29,'+ let i = -1;',C.green);if(scene==='approval')decisions(56,29,46);else put(77,29,'1 file · +3 −1',C.dim);}if(tab===2){put(6,27,'◒ cargo build --release',C.pink);put(6,29,'Compiling ratatui v0.30.2',C.dim);put(56,29,'Compiling winfer v0.1.73',C.dim);}if(tab===3)agentRows(6,27,96);
 prompt(33);status();}
function spine(){
 title('T inspect a plan step');
 const names=['Read format.ts','Rewrite unit loop','Build & test','Add TB cases'];
 rail(7,7,20);rail(30,4,27);put(3,4,'PLAN / 2 OF 4',C.violet,true);
 names.forEach((name,i)=>{const y=7+i*6,selected=i===state.tab;put(7,y,i<2?'✓':i===2?'◉':'○',selected?C.violet:C.dim);put(10,y,'0'+(i+1),selected?C.fg:C.dim,true);put(10,y+2,name,selected?C.fg:C.muted,selected);if(selected)label(3,y+3,'viewing',C.violet);});
 user(34,4,74);put(34,10,'✦',C.violet);summary(38,10,68);
 put(35,15,scene==='approval'?'PROPOSED CHANGE':`STEP 0${state.tab+1} / ${names[state.tab]}`,C.violet,true);
 if(scene==='approval'||state.tab===1){diff(35,18,71);if(scene==='approval')decisions(35,28,71);}
 else if(state.tab===0){read(35,18,71);put(38,26,'Related: format.test.ts · path.ts',C.dim);}
 else if(state.tab===2){box(34,18,74,10,'BUILD OUTPUT',true);put(37,20,'◒ cargo build --release',C.pink);put(37,23,'Compiling ratatui v0.30.2',C.dim);put(37,25,'Compiling winfer v0.1.73',C.dim);}
 else{put(35,19,'○ 1 TB boundary',C.muted);put(35,22,'○ values above 1 TB',C.muted);put(35,25,'○ preserves the existing signature',C.muted);}
 put(35,31,'Each plan step owns its tools and output.',C.dim);prompt(33);status();
}
function agentDeck(){
 title('T select an agent');
 const selected=state.agent;
 const agents=[['MAIN / OPUS','Implement unit-loop fix','◒ cargo build · 14s'],['EXPLORE / HAIKU','Find byte formatters','◒ 3 files read · 41s'],['REVIEW / HAIKU','Audit date formatting','◐ 7 tools · 41s']];
 agents.forEach(([name,task,activity],i)=>{const x=2+i*36;box(x,4,34,8,name,i===selected);put(x+3,6,task,C.muted);put(x+3,9,activity,i===selected?C.violet:C.dim);});
 user(2,14);put(2,19,'✦',C.violet);
 if(scene==='approval'){diff(6,19,100);decisions(6,29,100);}
 else{box(2,19,106,11,'SELECTED AGENT / '+agents[selected][0],true);
  if(selected===0){put(5,21,'src/lib/format.ts',C.fg,true);put(5,23,'+3 −1 · unit table and template strings',C.green);put(5,26,'Compiling ratatui v0.30.2',C.dim);put(5,28,'Compiling winfer v0.1.73',C.dim);}
  if(selected===1){put(5,21,'Other byte formatters found',C.fg,true);put(5,24,'◇ src/lib/format.ts',C.blue);put(5,26,'◇ src/lib/path.ts',C.blue);put(5,28,'No duplicate unit loop. Findings will return to the main agent.',C.dim);}
  if(selected===2){put(5,21,'Audit findings',C.fg,true);put(5,24,'✓ Date formatting does not share the byte unit loop.',C.green);put(5,26,'◒ Checking callers of formatBytes…',C.violet);put(5,28,'7 tools used · independent review channel',C.dim);}
 }
 plan(3,31,104);prompt(33);status();
}
function contextShelf(){
 title('T inspect attached context');user(2,4);put(2,10,'✦',C.violet);summary(6,10,99);
 if(scene==='approval'){diff(6,16,100);decisions(6,25,100);}
 else if(state.expanded){box(2,16,106,11,'CONTEXT PREVIEW / src/lib/format.ts',true);put(5,18,'Attached · 146 lines · refreshed after the last edit',C.dim);read(5,21,100);}
 else{read(6,16,100);working(6,23,100);put(6,25,'✓ CLAUDE.md loaded · 3 skills available · no stale attachments',C.dim);}
 box(2,28,106,5,'CONTEXT SHELF');label(5,29,'@ format.ts',C.blue);label(24,29,'@ format.test.ts',C.blue);label(49,29,'CLAUDE.md',C.muted);label(68,29,'3 skills',C.muted);put(87,29,'+ attach',C.violet);put(5,31,'2 files · 1.2k tokens',C.dim);put(50,31,'Files stay visible next to the prompt. T previews.',C.dim);
 prompt(34);status();
}
function checkpoints(){
 title('explicit hand-offs');
 rule(2,4,106);put(4,4,' ✓ understand ',C.dim);put(25,4,' ✓ propose ',C.dim);label(43,4,scene==='approval'?'◇ approve':'✓ approve',scene==='approval'?C.violet:C.dim);label(64,4,scene==='approval'?'○ execute':'◔ execute',scene==='approval'?C.dim:C.violet);put(88,4,' ○ verify ',C.dim);
 user(2,7);put(2,13,'✦',C.violet);summary(6,13,99);
 box(2,17,106,15,scene==='approval'?'CHECKPOINT / WRITE PERMISSION':'CHECKPOINT / EXECUTION',true);
 if(scene==='approval'||state.expanded){diff(5,19,100);if(scene==='approval'){put(5,28,'Scope: edit src/lib/format.ts only',C.muted);decisions(5,30,100);}else put(5,29,'✓ Patch accepted · build is running · tests follow next',C.muted);}
 else{put(5,20,'Build and test the unit-loop fix',C.fg,true);put(5,23,'✓ Scope locked: format.ts and its regression tests',C.muted);put(5,25,'◒ cargo build --release',C.violet);put(5,27,'○ pnpm vitest run src/lib',C.dim);put(5,29,'T inspect patch     ·     no changes outside the agreed scope',C.dim);}
 prompt(34);status();
}
function focusViews(){
 title('T switch full-width views');
 const view=scene==='approval'?2:scene==='conversation'?0:state.focus;
 const names=['Conversation','Execution','Changes'];let x=3;
 names.forEach((name,i)=>{label(x,4,name,view===i?C.bg:C.dim,view===i?C.violet:C.bg);x+=name.length+10;});rule(2,6,106);
 if(view===0){user(2,9);put(2,16,'✦',C.violet);summary(6,16,99);put(6,21,'What changed',C.fg,true);put(6,24,'• format.ts — rewritten loop, unit table, template strings',C.muted);put(6,26,'• format.test.ts — regression coverage for TB values',C.muted);put(6,29,'Execution details live in their own view. The conversation stays readable.',C.dim);}
 if(view===1){put(4,9,'EXECUTION / 3 TOOLS + 2 AGENTS',C.violet,true);read(5,12,100);put(5,20,'◒  ❯ bash  cargo build --release',C.pink);put(8,22,'Compiling ratatui v0.30.2',C.dim);put(8,23,'Compiling winfer v0.1.73',C.dim);rule(4,26,102);agentRows(5,28,100);}
 if(view===2){put(4,9,'CHANGES / 1 FILE',C.violet,true);diff(4,12,102);put(5,23,'Verification: '+(scene==='conversation'?'42 tests passed':'build in progress · tests pending'),C.muted);if(scene==='approval')decisions(5,28,100);else put(5,28,'Working tree · +3 −1 · no new dependencies',C.dim);}
 prompt(33);status();
}
export function proposal(number,id,base,options){if(id==='welcome'||id==='model-picker')return base;cells=blank();scene=id;state=options;[dock,groups,context,reviewTray,drawer,spine,agentDeck,contextShelf,checkpoints,focusViews][number-1]();return cells;}
