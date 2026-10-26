import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { cosmicProposal } from './cosmos.js';
import { spaceLayout } from './space-layouts.js';

const captures=JSON.parse(readFileSync(new URL('./scenes.json',import.meta.url),'utf8'));
function decode(frame){return frame.map(row=>{const cells=Array.from({length:110},()=>({ch:' ',fg:'#ececf2',bg:'#0e0e13',bold:false,italic:false,ul:''}));for(const [col,s,fg,bg,m,ul] of row){let x=col;for(const ch of s)cells[x++]={ch,fg:fg||'#ececf2',bg:bg||'#0e0e13',bold:!!(m&1),italic:!!(m&2),ul};}return cells;});}
const bases=Object.fromEntries(captures.map(s=>[s.id,decode(s.frames.at(-1))]));
const render=(id,options={})=>cosmicProposal(id,bases[id],{time:3,expanded:false,decision:'',...options});
const text=grid=>grid.map(row=>row.map(c=>c.ch).join('')).join('\n');

test('every scene and expanded state fits the terminal with valid colors',()=>{
 for(const id of Object.keys(bases))for(const expanded of [false,true])for(const time of [0,3,8.4,11.9,30]){
  const grid=render(id,{expanded,time});assert.equal(grid.length,40);
  for(const row of grid){assert.equal(row.length,110);for(const c of row){assert.equal([...c.ch].length,1);assert.match(c.fg,/^#[a-f\d]{6}$/i);assert.match(c.bg,/^#[a-f\d]{6}$/i);}}
 }
});
test('rendering is deterministic and animations change the frame',()=>{
 assert.deepEqual(render('working'),render('working'));
 assert.notDeepEqual(render('working',{time:0}),render('working',{time:2}));
});
test('effects do not animate the assistant reading background',()=>{
 const a=render('working',{time:0}),b=render('working',{time:9});
 for(let x=11;x<100;x++)assert.equal(a[15][x].bg,b[15][x].bg);
});
test('the original welcome wordmark and model picker are retained',()=>{
 const welcome=render('welcome');
 for(const y of [9,10])for(let x=0;x<110;x++)if(bases.welcome[y][x].ch!==' ')assert.equal(welcome[y][x].ch,bases.welcome[y][x].ch);
 assert.equal(render('model-picker'),bases['model-picker']);
});
test('tool expansion, inline approval, and a send pulse are visible',()=>{
 assert.notEqual(text(render('working')),text(render('working',{expanded:true})));
 assert.match(text(render('approval',{decision:'Approved once'})),/Approved once/);
 assert.match(text(render('working',{sentAt:2.5})),/Message queued/);
 assert.doesNotMatch(text(render('working',{sentAt:0})),/Message queued/);
});
test('ten space chat layouts change component structure, not just colors',()=>{
 const layouts=Array.from({length:10},(_,i)=>text(spaceLayout(i+1,'working',bases.working,{time:0,expanded:false})));
 assert.equal(new Set(layouts).size,10);
 for(const layout of layouts){assert.match(layout,/Ask anything/);assert.match(layout,/src\/lib\/format.ts/);assert.doesNotMatch(layout,/ACTIVITY DRAWER|CONTEXT RAIL|PLAN \/|AGENT DECK/);}
});
test('all new layout states fit, expand tools, and expose inline approval',()=>{
 for(let n=1;n<=10;n++){
  for(const id of Object.keys(bases))for(const expanded of [false,true]){
   const grid=spaceLayout(n,id,bases[id],{time:4,expanded,decision:''});
   assert.equal(grid.length,40);for(const row of grid){assert.equal(row.length,110);for(const cell of row)assert.match(cell.bg,/^#[a-f\d]{6}$/i);}
  }
  const collapsed=spaceLayout(n,'working',bases.working,{time:0,expanded:false});
  const expanded=spaceLayout(n,'working',bases.working,{time:0,expanded:true});
  assert.notEqual(text(collapsed),text(expanded));
  assert.match(text(spaceLayout(n,'approval',bases.approval,{time:0,decision:'Approved once'})),/Approved once/);
  assert.notDeepEqual(collapsed,spaceLayout(n,'working',bases.working,{time:1,expanded:false}));
  assert.match(text(spaceLayout(n,'working',bases.working,{time:1,sentAt:.5})),/Message queued/);
 }
});
test('composer animation never overwrites its label',()=>{
 for(let i=0;i<120;i++){
  const grid=spaceLayout(1,'working',bases.working,{time:i/10,expanded:false});
  assert.equal(grid[31].slice(9,16).map(c=>c.ch).join(''),' REPLY ');
 }
});
