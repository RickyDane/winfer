// Five presentations of the same simple, single-column terminal conversation.
const W=110,H=40;
const DEFAULT={bg:'#0e0e13',fg:'#ececf2',muted:'#a9a9af',dim:'#7d7d83',line:'#434349',panel:'#1b1b20',soft:'#15151d',violet:'#a98bff',blue:'#6fb6ff',pink:'#ff7ac6',green:'#5ee6c4',red:'#ff6b81',claude:'#ffa07a'};
let C=DEFAULT;
const question='formatBytes prints NaN for files over 1 TB, can you fix it and add tests?';
const answer='I’ll fix the unit loop, add TB test cases, and verify the existing API stays unchanged.';
const completed='Fixed. The unit loop now uses a unit table and template strings. TB values no longer print NaN.';
let grid,scene,state;
function put(x,y,text,fg=C.fg,bold=false,bg){for(const ch of text){if(grid[y]?.[x])grid[y][x]={...grid[y][x],ch,fg,bold,...(bg?{bg}:{})};x++;}}
function fill(x,y,w,h,bg){for(let j=y;j<y+h;j++)for(let i=x;i<x+w;i++)if(grid[j]?.[i])grid[j][i]={...grid[j][i],ch:' ',bg};}
function rule(x,y,w,color=C.line){put(x,y,'─'.repeat(w),color);}
function vertical(x,y,h,color=C.line){for(let j=y;j<y+h;j++)put(x,j,'│',color);}
function box(x,y,w,h,color=C.line){rule(x+1,y,w-2,color);rule(x+1,y+h-1,w-2,color);vertical(x,y+1,h-2,color);vertical(x+w-1,y+1,h-2,color);put(x,y,'╭',color);put(x+w-1,y,'╮',color);put(x,y+h-1,'╰',color);put(x+w-1,y+h-1,'╯',color);}
function text(x,y,s,w=96,color=C.fg,bold=false){let line='',row=y;for(const word of s.split(' ')){if(line&&line.length+word.length+1>w){put(x,row++,line,color,bold);line='';}line+=(line?' ':'')+word;}if(line)put(x,row,line,color,bold);return row+1;}
function response(x,y,w){return text(x,y,scene==='conversation'?completed:answer,w);}
function header(){put(4,1,'✦ winfer code',C.violet,true);put(24,1,'winfer / main',C.dim);}
function footer(){put(4,39,'▐ Claude Code ▌',C.claude);put(22,39,'Opus 4.5 · high · ask',C.muted);put(77,39,'ctx 23%    ctrl+t model',C.dim);}
function activity(x,y,w){put(x,y,scene==='conversation'?'✓ Complete · 1m 14s':'✺ Orbiting… · 1m 14s',scene==='conversation'?C.green:C.violet);put(x+w-11,y,scene==='conversation'?'42 passed':'esc to stop',C.dim);}
function tools(x,y,w){
 put(x,y,'✓ read',C.blue);put(x+9,y,'src/lib/format.ts',C.muted);put(x+w-13,y,'0.1s · '+(state.expanded?'▾':'▸'),C.dim);
 let row=y+2;
 if(state.expanded){fill(x,row,w,4,C.soft);vertical(x,row,4);put(x+3,row,'export function formatBytes(n: number) {',C.dim);put(x+3,row+1,'  if (n < 1024) return n + " B";',C.dim);put(x+3,row+2,'  do { n /= 1024; } while (n > 1024);',C.dim);put(x+3,row+3,'}',C.dim);row+=5;}
 put(x,row,scene==='conversation'?'✓ bash':'◒ bash',scene==='conversation'?C.green:C.pink);put(x+9,row,'cargo build --release',C.muted);put(x+w-13,row,scene==='conversation'?'done':'14s',C.dim);
 put(x,row+2,scene==='conversation'?'✓ 42 tests passed · 3 new regression cases':'Compiling winfer v0.1.73…',C.dim);
 return row+3;
}
function patch(x,y,w){put(x,y,'◇ edit',C.violet);put(x+9,y,'src/lib/format.ts',C.muted);put(x+w-8,y,'+3 −1',C.green);
 const lines=[['− if (n < 1024) return n + " B";',C.red,C.del||'#291921'],['+ if (n < 1024) return `${n} B`;',C.green,C.add||'#172a26'],['+ const units = ["KB", "MB", "GB"];',C.green,C.add||'#172a26'],['+ let i = -1;',C.green,C.add||'#172a26']];
 lines.forEach(([s,fg,bg],i)=>{fill(x,y+2+i,w,1,bg);put(x+2,y+2+i,s,fg);});
 put(x,y+7,'Allow this edit?',C.fg,true);
 if(state.decision){put(x,y+9,'✓ '+state.decision,C.green);put(x+25,y+9,'preview only',C.dim);}else{put(x,y+9,' Y approve once ',C.bg,true,C.violet);put(x+21,y+9,'A always allow',C.muted);put(x+w-8,y+9,'N deny',C.muted);}
}
function open(){
 header();fill(4,5,102,3,C.panel);put(6,5,'you',C.dim);text(6,6,question,98,C.fg,true);
 put(4,11,'✦',C.violet);put(7,11,'winfer',C.violet,true);put(17,11,'thought for 4s',C.dim);response(7,13,97);
 if(scene==='approval')patch(7,17,97);else tools(7,17,97);
 activity(4,31,102);rule(4,33,102,C.violet);put(4,35,'❯',C.violet,true);put(7,35,'Ask anything, or steer the agent…',C.dim);put(4,37,'@ files   / commands   ! shell',C.dim);put(78,37,'⇧tab mode   ↵ send',C.dim);footer();
}
function soft(){
 header();fill(24,4,82,5,C.panel);box(24,4,82,5,'#34313f');put(27,5,'you',C.dim);text(27,6,question,76,C.fg,true);
 fill(4,12,96,17,C.soft);put(7,13,'✦ winfer',C.violet,true);put(76,13,'thought for 4s',C.dim);response(7,15,89);
 if(scene==='approval')patch(7,18,89);else tools(7,19,89);
 fill(4,32,102,6,C.panel);vertical(4,32,6,C.violet);put(7,33,'❯',C.violet,true);put(10,33,'Ask anything, or steer the agent…',C.dim);activity(7,36,96);footer();
}
function editorial(){
 put(12,1,'✦ winfer code',C.violet,true);put(77,1,'winfer / main',C.dim);
 put(12,5,'YOU',C.muted,true);put(90,5,'14:32',C.dim);rule(12,6,86);text(12,8,question,86,C.fg,true);
 put(12,13,'WINFER',C.violet,true);put(80,13,'thought for 4s',C.dim);rule(12,14,86);response(12,16,86);
 if(scene==='approval')patch(12,20,86);else tools(12,20,86);
 rule(12,32,86,C.violet);put(12,34,'›',C.violet,true);put(15,34,'Ask anything…',C.dim);put(12,36,'@ attach   / commands',C.dim);put(68,36,'Opus 4.5 · high · ask',C.muted);put(12,38,scene==='conversation'?'✓ Complete · 42 tests passed':'✺ Orbiting… · esc to stop',C.violet);put(83,38,'ctx 23%',C.dim);
}
function thread(){
 header();vertical(5,5,27,'#373140');put(5,5,'●',C.violet);put(9,5,'you',C.muted,true);text(9,7,question,95,C.fg,true);
 put(5,12,'✦',C.violet);put(9,12,'winfer',C.violet,true);put(21,12,'thought for 4s',C.dim);response(9,14,95);
 if(scene==='approval'){put(5,18,'◇',C.violet);patch(9,18,95);}else{put(5,19,'◇',C.blue);tools(9,19,95);}
 put(5,32,'╰',C.violet);rule(6,32,100,C.violet);put(9,34,'❯',C.violet,true);put(12,34,'Ask anything, or steer the agent…',C.dim);activity(9,36,95);footer();
}
function compact(){
 header();rule(4,3,102);put(4,6,'you',C.muted,true);text(16,6,question,88,C.fg,true);
 put(4,11,'winfer',C.violet,true);response(16,11,88);put(16,14,'thought for 4s',C.dim);
 if(scene==='approval'){put(4,17,'edit',C.violet);patch(16,17,88);}else{put(4,17,'tools',C.dim);tools(16,17,88);}
 rule(4,30,102);activity(4,31,102);box(4,33,102,4,C.violet);put(7,34,'❯',C.violet,true);put(10,34,'Ask anything, or steer the agent…',C.dim);put(7,36,' @ files   / commands   ! shell ',C.dim);put(81,36,' ↵ send ',C.violet);footer();
}
export function proposal(number,id,base,options){
 if(id==='welcome'||id==='model-picker')return base;
 C=DEFAULT;
 grid=Array.from({length:H},()=>Array.from({length:W},()=>({ch:' ',fg:C.fg,bg:C.bg,bold:false,italic:false,ul:''})));
 scene=id;state=options;[open,soft,editorial,thread,compact][number-1]();return grid;
}

const palettes=[
 {bg:'#0b0c0e',fg:'#ededeb',muted:'#b9b9b6',dim:'#858583',line:'#444542',panel:'#e6e6e0',soft:'#18191b',violet:'#e6e6e0',blue:'#b9b9b6',pink:'#dededb',green:'#abd8bd',red:'#e6a2a4',claude:'#b9b9b6'},
 {bg:'#f0eadf',fg:'#302e2b',muted:'#625e55',dim:'#797267',line:'#c6bdad',panel:'#e4dbcc',soft:'#e8e0d3',violet:'#715642',blue:'#385f77',pink:'#855a46',green:'#35674a',red:'#a33d3d',claude:'#715642',add:'#dbe6d5',del:'#ead6ce'},
 {bg:'#101914',fg:'#dce7d8',muted:'#acbda5',dim:'#849480',line:'#3b5040',panel:'#213126',soft:'#18241c',violet:'#a8bd8e',blue:'#85aea2',pink:'#d7bf91',green:'#afd39b',red:'#e8a29c',claude:'#a8bd8e',add:'#233723',del:'#342323'},
 {bg:'#0c192b',fg:'#d5e7f5',muted:'#a3bed4',dim:'#7c9bb7',line:'#29496a',panel:'#112b44',soft:'#102236',violet:'#7dcff5',blue:'#7dcff5',pink:'#b3c7fa',green:'#94dfbf',red:'#f6a2ad',claude:'#a3bed4',add:'#16342f',del:'#342436'},
 {bg:'#211b20',fg:'#f0e1dc',muted:'#c1aba9',dim:'#a38c90',line:'#604950',panel:'#39272e',soft:'#2b2026',violet:'#f3b484',blue:'#d7b6c3',pink:'#f3b484',green:'#bed6a5',red:'#f1a3a2',claude:'#f3b484',add:'#2c3829',del:'#4a282f'},
];
function square(x,y,w,h){rule(x+1,y,w-2);rule(x+1,y+h-1,w-2);vertical(x,y+1,h-2);vertical(x+w-1,y+1,h-2);put(x,y,'┌',C.line);put(x+w-1,y,'┐',C.line);put(x,y+h-1,'└',C.line);put(x+w-1,y+h-1,'┘',C.line);}
function content(x,y,w){if(scene==='approval')patch(x,y,w);else tools(x,y,w);}
function ink(){
 put(4,1,'WINFER CODE',C.fg,true);put(86,1,'winfer / main',C.dim);
 fill(4,4,102,5,C.panel);put(7,5,'YOU',C.bg,true);text(7,7,question,96,C.bg,true);
 put(4,12,'WINFER',C.fg,true);put(17,12,'/ thought for 4s',C.dim);response(4,14,102);content(4,18,102);
 activity(4,31,102);rule(4,33,102,C.fg);put(4,35,'>',C.fg,true);put(7,35,'Write a message…',C.muted);put(4,37,'@ attach   / commands',C.dim);put(83,37,'ENTER to send',C.dim);footer();
}
function paper(){
 put(10,1,'✦ winfer code / field notes',C.fg,true);put(83,1,'main · 01',C.dim);
 put(10,5,'YOU',C.violet,true);put(87,5,'14:32',C.dim);rule(10,6,90);text(10,8,question,90,C.fg,true);
 put(10,13,'WINFER',C.violet,true);put(76,13,'thought for 4s',C.dim);rule(10,14,90);response(10,16,90);content(10,20,90);
 fill(10,32,90,6,C.panel);rule(10,32,90,C.violet);put(13,34,'>',C.violet,true);put(16,34,'Write a message…',C.muted);put(13,36,'@ attach   / commands',C.dim);put(80,36,'↵ send',C.violet);footer();
}
function moss(){
 put(4,1,'✦ winfer',C.violet,true);put(16,1,'/ code',C.dim);put(83,1,'main · Opus 4.5',C.dim);rule(4,3,102);
 put(4,6,'you',C.muted,true);text(16,6,question,90,C.fg,true);
 put(4,11,'winfer',C.violet,true);response(16,11,90);put(16,14,'thought for 4s',C.dim);content(16,18,90);
 square(4,32,102,6);put(7,32,' message ',C.violet,true,C.bg);put(7,34,'❯',C.violet,true);put(10,34,'Write a message…',C.muted);put(7,36,'/ commands   @ attach',C.dim);put(76,36,'high · ask   ↵ send',C.dim);footer();
}
function blueprint(){
 put(4,1,'WINFER // CODE',C.violet,true);put(78,1,'WORKSPACE / MAIN',C.dim);square(4,4,102,5);put(7,4,' YOU ',C.violet,true,C.bg);text(7,6,question,96,C.fg,true);
 rule(4,12,102);put(7,12,' AGENT / OPUS 4.5 ',C.violet,true,C.bg);response(7,14,96);content(7,18,96);
 activity(4,30,102);square(4,33,102,5);put(7,33,' MESSAGE ',C.violet,true,C.bg);put(7,35,'>',C.violet,true);put(10,35,'Write a message…',C.muted);put(78,35,'@ FILES  / HELP',C.dim);footer();
}
function ember(){
 put(8,1,'✦ winfer code',C.violet,true);put(84,1,'winfer / main',C.dim);
 fill(28,5,74,5,C.panel);put(31,6,'you',C.muted);text(31,7,question,68,C.fg,true);
 put(8,14,'✦',C.violet);put(12,14,'winfer',C.violet,true);put(79,14,'thought for 4s',C.dim);response(12,16,90);content(12,20,90);
 fill(8,33,94,5,C.panel);put(11,34,'›',C.violet,true);put(14,34,'Write a message…',C.muted);put(11,36,'@ attach   / commands',C.dim);put(75,36,'↵ send',C.violet);footer();
}
export function styledProposal(number,id,base,options){
 if(id==='welcome'||id==='model-picker')return base;
 C=palettes[number-1];scene=id;state=options;
 grid=Array.from({length:H},()=>Array.from({length:W},()=>({ch:' ',fg:C.fg,bg:C.bg,bold:false,italic:false,ul:''})));
 [ink,paper,moss,blueprint,ember][number-1]();return grid;
}
