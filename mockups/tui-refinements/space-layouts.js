import { cosmicProposal, spaceFoundation, spaceColors as C, spectrum, orbit } from './cosmos.js';

const question='formatBytes prints NaN for files over 1 TB, can you fix it and add tests?';
const answer='I’ll fix the unit loop, add TB test cases, and verify the existing API stays unchanged.';
const completed='Fixed. The unit loop now uses a unit table and template strings. TB values no longer print NaN.';

export function spaceLayout(number,id,base,options){
 if(id==='welcome'||id==='model-picker')return cosmicProposal(id,base,options);
 const t=options.time||0,grid=spaceFoundation(t),busy=id!=='conversation',sending=options.sentAt!==undefined&&t-options.sentAt<2;
 // Let the new components define their own edges; keep the reading surface quiet.
 for(let y=4;y<33;y++)for(const cell of grid[y])cell.bg=C.bg;
 const spin=orbit[Math.floor(t*6)%orbit.length];
 function put(x,y,s,fg=C.fg,bold=false,bg){for(const ch of s){if(grid[y]?.[x])grid[y][x]={...grid[y][x],ch,fg,bold,...(bg?{bg}:{})};x++;}}
 function fill(x,y,w,h,bg=C.panel){for(let j=y;j<y+h;j++)for(let i=x;i<x+w;i++)if(grid[j]?.[i])grid[j][i]={...grid[j][i],ch:' ',bg};}
 function line(x,y,w,color=C.line){put(x,y,'─'.repeat(w),color);}
 function rail(x,y,h,color=C.line){for(let j=y;j<y+h;j++)put(x,j,'│',color);}
 function words(x,y,s,w=90,fg=C.fg,bold=false){let row=y,part='';for(const word of s.split(' ')){if(part&&part.length+word.length+1>w){put(x,row++,part,fg,bold);part='';}part+=(part?' ':'')+word;}if(part)put(x,row,part,fg,bold);}
 function tag(x,y,s,fg=C.muted,bg=C.panel){put(x,y,' '+s+' ',fg,true,bg);}
 function frame(x,y,w,h,label='',active=false){
  for(let i=1;i<w-1;i++){put(x+i,y,'─',active?spectrum(i,t):C.line);put(x+i,y+h-1,'─',C.line);}
  rail(x,y+1,h-2);rail(x+w-1,y+1,h-2);put(x,y,'╭',active?spectrum(0,t):C.line);put(x+w-1,y,'╮',active?spectrum(w,t):C.line);put(x,y+h-1,'╰',C.line);put(x+w-1,y+h-1,'╯',C.line);
  if(label)put(x+3,y,' '+label+' ',active?C.violet:C.muted,true,C.bg);
 }
 function head(name,x=6){put(x,1,'✦ winfer',C.violet,true);put(x+11,1,'/ '+name,C.muted);put(83,1,'⎇ main',C.dim);}
 function response(x,y,w){words(x,y,busy?answer:completed,w);}
 function activity(x,y,w){put(x,y,sending?'✧ Message queued · preview only':busy?spin+' Orbiting… · 1m 14s':'✓ Complete · 42 tests passed',sending?C.cyan:busy?C.violet:C.green);put(x+w-11,y,busy?'esc to stop':'1m 14s',C.dim);}
 function metadata(y=39,x=6){put(x,y,'Claude Code',C.muted);put(x+15,y,'Opus 4.5 · high · ask',C.dim);put(80,y,'ctx ▰▰▱▱▱▱ 23%',C.dim);}
 function receipts(x,y,w){
  put(x,y,'◇ read',C.cyan);put(x+9,y,'src/lib/format.ts',C.muted);put(x+w-10,y,'0.1s '+(options.expanded?'▾':'▸'),C.dim);
  if(options.expanded){fill(x+2,y+2,w-2,4,'#10192d');rail(x,y+2,4);put(x+4,y+2,'export function formatBytes(n: number) {',C.dim);put(x+4,y+3,'  if (n < 1024) return n + " B";',C.dim);put(x+4,y+4,'  do { n /= 1024; } while (n > 1024);',C.dim);put(x+4,y+5,'}',C.dim);}
  const yy=y+(options.expanded?7:2);put(x,yy,busy?spin+' bash':'✓ bash',C.pink);put(x+9,yy,'cargo build --release',C.muted);put(x+w-10,yy,busy?'14s':'done',C.dim);put(x+9,yy+2,busy?'Compiling winfer v0.1.73…':'42 tests passed · 3 new cases',C.dim);
 }
 function approval(x,y,w){
  put(x,y,'◇ edit',C.violet);put(x+9,y,'src/lib/format.ts',C.muted);put(x+w-7,y,'+3 −1',C.green);
  const patch=[['− if (n < 1024) return n + " B";',C.red,'#281c30'],['+ if (n < 1024) return `${n} B`;',C.green,'#112b2e'],['+ const units = ["KB", "MB", "GB"];',C.green,'#112b2e'],['+ let i = -1;',C.green,'#112b2e']];
  patch.forEach(([s,fg,bg],i)=>{fill(x,y+2+i,w,1,bg);put(x+2,y+2+i,s,fg);});
  put(x,y+7,'Allow this edit?',C.muted);
  if(options.decision){put(x,y+9,'✓ '+options.decision,C.green);put(x+27,y+9,'preview only',C.dim);}else{tag(x,y+9,'Y approve once',C.bg,C.cyan);put(x+22,y+9,'A always allow',C.muted);put(x+w-7,y+9,'N deny',C.muted);}
 }
 function tools(x,y,w){if(id==='approval')approval(x,y,w);else receipts(x,y,w);}
 function glint(x,y,w){const n=Math.floor((sending?(t-options.sentAt)*72:t*7)%w);put(x+n,y,'─',sending?C.fg:C.cyan);}

 function capsule(){
  head('capsule');frame(22,4,82,5,'YOU / 01');words(25,6,question,76,C.fg,true);
  frame(6,12,98,18,'✦ WINFER / REPLY 01',true);put(80,12,' thought 4s ',C.dim,false,C.bg);response(9,14,92);tools(9,18,92);
  frame(6,31,98,7,'REPLY',true);tag(10,32,'@ format.ts',C.cyan);put(10,34,'❯',C.cyan,true);put(13,34,'Ask anything, or steer the agent…',C.muted);tag(10,36,'Claude Code · Opus 4.5',C.muted,'#202943');put(48,36,'high · ask',C.dim);put(84,36,'ctx 23%',C.dim);glint(20,31,80);
  put(8,39,sending?'✧ Message queued · preview only':'T expand tools   ·   @ attach   ·   ↵ send',sending?C.cyan:C.dim);
 }
 function transmission(){
  head('transmission');line(6,4,98);put(9,4,' 01 ↑ YOU ',C.cyan,true,C.bg);words(9,6,question,92,C.fg,true);
  line(6,11,98);put(9,11,' 02 ↓ WINFER ',C.violet,true,C.bg);put(82,11,' thought 4s ',C.dim,false,C.bg);response(9,13,92);tools(9,18,92);activity(6,30,98);
  line(6,32,98,C.violet);put(9,32,' UPLINK ',C.violet,true,C.bg);glint(20,32,80);fill(0,33,110,5,'#131e37');put(6,34,'❯',C.cyan,true);put(9,34,'Ask anything, or steer the agent…',C.muted);put(6,36,'@ attach   / commands',C.dim);put(79,36,'↵ transmit',C.cyan);metadata();
 }
 function orbitChat(){
  head('orbit');rail(7,5,28,'#3b3d68');put(7,5,'○',C.cyan);put(12,5,'you',C.muted,true);words(12,7,question,90,C.fg,true);
  put(7,12,busy?spin:'✦',C.violet);put(12,12,'winfer',C.violet,true);put(24,12,'thought for 4s',C.dim);response(12,14,90);put(7,19,'◇',C.cyan);tools(12,19,90);
  put(7,33,'╰',C.violet);for(let i=8;i<103;i++)put(i,33,'─',spectrum(i,t));put(12,35,'❯',C.cyan,true);put(15,35,'Ask anything, or steer the agent…',C.muted);activity(12,31,90);put(12,37,'@ attach    / commands',C.dim);put(79,37,'↵ send',C.cyan);metadata();
 }
 function logbook(){
  head('starlog',14);put(14,5,'001 / YOU',C.cyan,true);put(87,5,'14:32',C.dim);line(14,6,82);words(14,8,question,82,C.fg,true);
  put(14,13,'002 / WINFER',C.violet,true);put(78,13,'thought 4s',C.dim);response(14,15,82);tools(14,19,82);
  fill(14,32,82,6,'#10192d');put(17,32,'NEW ENTRY',C.violet,true);line(29,32,67,C.line);glint(30,32,64);put(17,34,'›',C.cyan,true);put(20,34,'Ask anything…',C.muted);put(17,36,sending?'✧ Message queued · preview only':'@ attach   / commands',sending?C.cyan:C.dim);put(77,36,'↵ send',C.cyan);metadata();
 }
 function horizon(){
  head('horizon',4);put(4,5,'YOU',C.cyan,true);words(4,7,question,102,C.fg,true);
  put(4,12,busy?spin+' winfer':'✦ winfer',C.violet,true);put(85,12,'thought 4s',C.dim);response(4,14,102);
  if(id==='approval')approval(4,18,102);else{frame(4,18,102,12,'TOOLS / '+(options.expanded?'EXPANDED':'2 CALLS'));receipts(7,19,96);}
  fill(0,32,110,8,'#131d34');for(let i=0;i<110;i++)put(i,32,'─',spectrum(i,t));glint(1,32,107);tag(4,33,'Claude Code · Opus 4.5',C.cyan,'#23314c');put(38,33,'high · ask',C.muted);put(85,33,'ctx 23%',C.dim);
  put(4,35,'❯',C.cyan,true);put(8,35,'Ask anything, or steer the agent…',C.muted);put(4,37,sending?'✧ Message queued · preview only':'↳ also update the changelog · queued',sending?C.cyan:C.dim);tag(4,39,'@ format.ts',C.muted,'#202b43');put(26,39,'/ commands   ! shell',C.dim);put(88,39,'↵ send',C.cyan);
 }
 function lunar(){
  head('lunar',10);put(10,6,'○',C.cyan,true);put(18,4,'YOU',C.dim);words(18,6,question,84,C.fg,true);line(18,9,84,C.line);
  const phase=['◔','◑','◕','●'][Math.floor(t*.6)%4];put(10,13,busy?phase:'●',C.violet,true);put(18,12,'winfer',C.violet,true);tag(29,12,'Opus 4.5',C.muted,'#202743');put(86,12,'thought 4s',C.dim);response(18,14,84);tools(18,19,84);
  frame(18,33,84,5,'',true);put(10,35,'◌',spectrum(0,t),true);put(21,35,'Ask anything…',C.muted);put(88,35,'↵ send',C.cyan);put(21,37,' @ attach ',C.dim,false,C.bg);put(72,37,' high · ask · 23% ',C.dim,false,C.bg);glint(20,33,80);
  put(18,39,sending?'✧ Message queued · preview only':'T expand tools · model follows the assistant',sending?C.cyan:C.dim);
 }
 function relay(){
  head('relay');put(6,5,'YOU',C.cyan,true);words(6,7,question,98,C.fg,true);put(6,12,busy?spin+' winfer':'✦ winfer',C.violet,true);response(6,14,98);
  if(id==='approval')approval(6,18,98);else if(options.expanded)receipts(6,18,98);else{line(6,19,98);put(9,19,' THIS REPLY ',C.dim,false,C.bg);put(9,21,'◇ read src/lib/format.ts',C.cyan);put(35,21,'✓ edit +3 −1',C.green);put(60,21,busy?spin+' build running':'✓ 42 passed',C.pink);put(89,21,'T details',C.dim);}
  const label=busy?'QUEUED / AFTER THIS TURN':'NEXT TURN';put(24,29,'╭',C.line);put(25,29,'┄'.repeat(76),C.line);put(101,29,'╮',C.line);put(27,29,' '+label+' ',C.violet,true,C.bg);put(24,30,'┊',C.line);put(101,30,'┊',C.line);put(27,31,busy?'also update the changelog':'Ask a follow-up when you’re ready…',C.dim);line(25,32,76,C.line);put(24,32,'╰',C.line);put(101,32,'╯',C.line);glint(55,29,44);
  put(6,35,'❯',C.cyan,true);put(9,35,'Ask anything, or queue the next message…',C.muted);put(90,35,'↵ send',C.cyan);line(6,36,98,C.line);put(6,38,'Claude Code · Opus 4.5 · high · ask',C.dim);put(83,38,'ctx 23%',C.dim);if(sending)put(6,39,'✧ Message queued · preview only',C.cyan);
 }
 function aurora(){
  head('aurora');frame(6,4,98,7,'NEXT MESSAGE',true);put(9,6,'❯',C.cyan,true);put(12,6,'Ask anything, or steer the agent…',C.muted);put(90,6,'↵ send',C.cyan);tag(9,9,'Claude Code · Opus 4.5',C.muted,'#202943');put(47,9,'@ format.ts',C.cyan);put(81,9,'ctx 23%',C.dim);glint(28,4,73);
  put(6,13,busy?spin+' WINFER / LATEST REPLY':'✦ WINFER / LATEST REPLY',C.violet,true);put(88,13,'thought 4s',C.dim);response(6,15,98);tools(6,19,98);
  line(6,32,98);put(9,32,' EARLIER / YOU ',C.dim,false,C.bg);words(9,34,question,92,C.muted);put(6,38,sending?'✧ Message queued · preview only':'New messages start at the top; the current reply stays immediately below.',sending?C.cyan:C.dim);
 }
 function eclipse(){
  head('eclipse');fill(0,4,110,7,'#14223b');put(6,5,'○ YOU',C.cyan,true);words(6,7,question,98,C.fg,true);
  fill(0,11,110,20,C.bg);put(6,13,busy?spin+' WINFER':'✦ WINFER',C.violet,true);put(83,13,'thought 4s',C.dim);response(6,15,98);tools(6,20,98);
  for(let i=0;i<110;i++)put(i,32,'─',spectrum(i,t));put(53,32,['◔','◑','◕','●'][Math.floor(t*.6)%4],C.violet,true,C.bg);
  frame(6,34,98,4,'REPLY / OPUS 4.5',true);put(9,36,'❯',C.cyan,true);put(12,36,'Ask anything, or steer the agent…',C.muted);put(90,36,'↵ send',C.cyan);glint(31,34,70);put(6,39,sending?'✧ Message queued · preview only':'@ attach   / commands   ·   high / ask',sending?C.cyan:C.dim);put(83,39,'ctx 23%',C.dim);
 }
 function drift(){
  head('drift',12);put(60,2,'Opus 4.5 · high · ask',C.dim);put(91,2,'23%',C.dim);put(91,5,'YOU',C.dim);words(32,7,question,66,C.fg,true);line(32,10,66,C.line);
  put(8,14,busy?spin:'✦',C.violet,true);put(12,13,'winfer',C.violet,true);put(79,13,'thought 4s',C.dim);response(12,15,88);tools(12,20,88);
  for(let i=20;i<94;i++)put(i,33,'─',spectrum(i,t));glint(21,33,71);put(21,35,'›',C.cyan,true);put(24,35,'Ask anything…',C.muted);put(83,35,'↵ send',C.cyan);put(21,37,'@ attach    / commands',C.dim);
  put(12,39,sending?'✧ Message queued · preview only':busy?spin+' Orbiting… · 1m 14s':'✓ Complete · 42 tests passed',sending?C.cyan:C.dim);
 }
 const layouts=[capsule,transmission,orbitChat,logbook,horizon,lunar,relay,aurora,eclipse,drift];
 if(!layouts[number-1])throw new RangeError('Unknown space layout');
 layouts[number-1]();return grid;
}
