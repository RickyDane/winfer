// Terminal-native space treatment. Every visual is a colored text cell.
const W=110,H=40;
const C={bg:'#080c1a',fg:'#e0e8fc',muted:'#a4b1d0',dim:'#7584aa',line:'#344369',cyan:'#7bd9f5',violet:'#b99bfa',pink:'#e997cd',green:'#8cdcc4',red:'#f0a0b6',panel:'#151e36'};
const orbit=['⠋','⠙','⠹','⠸','⠼','⠴','⠦','⠧','⠇','⠏'];
const question='formatBytes prints NaN for files over 1 TB, can you fix it and add tests?';
const hash=n=>{const value=Math.sin(n*127.1+311.7)*43758.5453;return value-Math.floor(value);};
function blend(a,b,k){const rgb=s=>[1,3,5].map(i=>parseInt(s.slice(i,i+2),16)),A=rgb(a),B=rgb(b);return '#'+A.map((v,i)=>Math.round(v+(B[i]-v)*k).toString(16).padStart(2,'0')).join('');}
function nebula(x,y,t){const violet=Math.exp(-((x-8-Math.sin(t*.12)*4)**2/440+(y-14)**2/200));const blue=Math.exp(-((x-108)**2/460+(y-8-Math.cos(t*.1)*3)**2/140));return blend(blend(C.bg,'#302044',violet*.46),'#14314b',blue*.55);}
function spectrum(x,t){const phase=(Math.sin(x*.065-t*.42)+1)/2;return blend(C.violet,phase>.5?C.cyan:C.pink,Math.abs(phase-.5)*2);}
function sky(t,welcome){
 const out=Array.from({length:H},(_,y)=>Array.from({length:W},(_,x)=>({ch:' ',fg:C.fg,bg:!welcome&&x>7&&x<103&&y>4&&y<33?C.bg:nebula(x,y,t),bold:false,italic:false,ul:''})));
 for(let i=0;i<(welcome?205:95);i++){
  let x=Math.floor(hash(i+1)*W),y=Math.floor(hash(i+301)*H);
  if(!welcome&&x>5&&x<104&&y>3)continue;
  const pulse=(Math.sin(t*(.35+hash(i+900)*.5)+hash(i+700)*6.28)+1)/2;
  const ch=pulse>.93?'✧':hash(i+500)>.8?'·':'˙';
  out[y][x]={...out[y][x],ch,fg:blend(out[y][x].bg,i%3?C.violet:C.cyan,.13+pulse*.4)};
 }
 // A slow comet crosses only the sky, never the conversation or prompt.
 const progress=(t%12)/12;
 if(progress>.63&&progress<.82){const p=(progress-.63)/.19,x=Math.floor(104-p*78),y=Math.floor(1+p*3);for(let i=0;i<9;i++){const xx=x+i,yy=y-Math.floor(i/5);if(out[yy]?.[xx])out[yy][xx]={...out[yy][xx],ch:i===0?'✦':'─',fg:blend(C.bg,C.cyan,(1-i/9)*.75)};}}
 return out;
}
function planet(grid,t){
 const cx=88,cy=7;
 // Back half of the tilted ring.
 for(let x=-10;x<=10;x++){const y=Math.round(-x*.16);if(grid[cy+y]?.[cx+x])grid[cy+y][cx+x]={...grid[cy+y][cx+x],ch:'─',fg:'#465780'};}
 for(let y=-2;y<=2;y++)for(let x=-5;x<=5;x++)if(x*x/25+y*y/5.8<1){const light=(x+5)/10,color=blend('#45527f','#b4a5d9',1-light*.8);grid[cy+y][cx+x]={...grid[cy+y][cx+x],ch:Math.abs(y)===2?(y<0?'▄':'▀'):' ',fg:color,...(Math.abs(y)<2?{bg:color}:{})};}
 for(let x=-10;x<=10;x++){const y=Math.round(1-x*.16);if(x<-3||x>2)grid[cy+y][cx+x]={...grid[cy+y][cx+x],ch:x%3?'─':'·',fg:spectrum(x+40,t)};}
}
export { C as spaceColors, spectrum, orbit };
export function spaceFoundation(time=0){return sky(time,false);}
export function cosmicProposal(id,base,options){
 if(id==='model-picker')return base;
 const t=options.time||0,grid=sky(t,id==='welcome');
 function put(x,y,s,fg=C.fg,bold=false,bg){for(const ch of s){if(grid[y]?.[x])grid[y][x]={...grid[y][x],ch,fg,bold,...(bg?{bg}:{})};x++;}}
 function fill(x,y,w,h,bg){for(let j=y;j<y+h;j++)for(let i=x;i<x+w;i++)if(grid[j]?.[i])grid[j][i]={...grid[j][i],ch:' ',bg};}
 function text(x,y,s,w=88,color=C.fg){let line='',row=y;for(const word of s.split(' ')){if(line&&line.length+word.length+1>w){put(x,row++,line,color);line='';}line+=(line?' ':'')+word;}if(line)put(x,row,line,color);}
 function frame(x,y,w,h){for(let i=1;i<w-1;i++){put(x+i,y,'─',spectrum(i,t));put(x+i,y+h-1,'─',blend(C.bg,spectrum(i,t),.5));}for(let i=1;i<h-1;i++){put(x,y+i,'│',C.line);put(x+w-1,y+i,'│',C.line);}put(x,y,'╭',spectrum(0,t));put(x+w-1,y,'╮',spectrum(w,t));put(x,y+h-1,'╰',C.line);put(x+w-1,y+h-1,'╯',C.line);}
 if(id==='welcome'){
  planet(grid,t);
  // Keep the real welcome wordmark, shortcut cards, and composer.
  base.forEach((row,y)=>row.forEach((c,x)=>{if(c.ch===' '&&c.bg==='#0e0e13')return;if(/[·˙✦✧]/.test(c.ch)&&(y<9||y>22))return;const bg=c.bg==='#0e0e13'?grid[y][x].bg:c.bg;grid[y][x]={...c,bg};if(y>=35&&y<=37&&/[─│╭╮╰╯]/.test(c.ch))grid[y][x].fg=spectrum(x,t);}));
  return grid;
 }
 put(8,2,'✦',spectrum(0,t),true);put(11,2,'W I N F E R',C.fg,true);put(25,2,'/ COSMOS',C.violet);put(78,2,'winfer · ⎇ main',C.dim);
 // A small constellation, not an application navigation bar.
 put(46,2,'·',C.line);put(49,2,'──',C.line);put(53,2,'✧',C.violet);put(57,2,'───',C.line);put(63,2,'·',C.line);
 fill(20,6,82,4,C.panel);put(23,6,'you',C.dim);text(23,8,question,76);
 put(8,13,id==='conversation'?'✦':orbit[Math.floor(t*6)%orbit.length],spectrum(0,t),true);put(11,13,'winfer',C.violet,true);put(21,13,'thought for 4s',C.dim);
 text(11,15,id==='conversation'?'Fixed. The unit loop now uses a unit table and template strings. TB values no longer print NaN.':'I’ll fix the unit loop, add TB test cases, and verify the existing API stays unchanged.',90);
 if(id==='approval'){
  put(11,19,'◇ edit',C.violet);put(20,19,'src/lib/format.ts',C.muted);put(92,19,'+3 −1',C.green);
  const lines=[['− if (n < 1024) return n + " B";',C.red,'#281c30'],['+ if (n < 1024) return `${n} B`;',C.green,'#112b2e'],['+ const units = ["KB", "MB", "GB"];',C.green,'#112b2e'],['+ let i = -1;',C.green,'#112b2e']];
  lines.forEach(([s,fg,bg],i)=>{fill(11,21+i,90,1,bg);put(13,21+i,s,fg);});
  if(options.decision){put(11,27,'✓ '+options.decision,C.green);put(38,27,'preview only',C.dim);}else{put(11,27,' Y approve once ',C.bg,true,C.cyan);put(35,27,'A always allow',C.muted);put(92,27,'N deny',C.muted);}
 }else{
  put(11,20,'✓ read',C.cyan);put(20,20,'src/lib/format.ts',C.muted);put(90,20,'0.1s  '+(options.expanded?'▾':'▸'),C.dim);
  if(options.expanded){fill(11,22,90,4,'#10192d');put(13,22,'export function formatBytes(n: number) {',C.dim);put(13,23,'  if (n < 1024) return n + " B";',C.dim);put(13,24,'  do { n /= 1024; } while (n > 1024);',C.dim);put(13,25,'}',C.dim);}
  const y=options.expanded?27:23;put(11,y,id==='conversation'?'✓ bash':orbit[Math.floor(t*5)%orbit.length]+' bash',C.pink);put(20,y,'cargo build --release',C.muted);put(90,y,id==='conversation'?'done':'14s',C.dim);put(20,y+2,id==='conversation'?'42 tests passed · 3 new cases':'Compiling winfer v0.1.73…',C.dim);
 }
 const pulse=options.sentAt!==undefined&&t-options.sentAt<2;
 put(9,31,pulse?'✧ Message queued · preview only':id==='conversation'?'✓ Complete · 42 tests passed':'✺ Orbiting… · 1m 14s',pulse?C.cyan:C.violet);put(90,31,'esc to stop',C.dim);
 fill(8,33,94,5,'#0e1529');frame(8,33,94,5);
 put(11,35,'❯',C.cyan,true);put(14,35,'Ask anything, or steer the agent…',C.muted);put(11,37,' @ attach   / commands ',C.dim,false,'#0e1529');put(80,37,' ↵ send ',C.cyan,false,'#0e1529');
 // A quiet traveling glint; Enter launches a faster pulse along the same border.
 const glint=Math.floor((pulse?(t-options.sentAt)*72:t*7)%90);put(10+glint,33,'─',pulse?C.fg:C.cyan);
 put(8,39,'Claude Code',C.muted);put(23,39,'Opus 4.5 · high · ask',C.dim);put(78,39,'ctx ▰▰▱▱▱▱▱▱ 23%',C.dim);
 return grid;
}
