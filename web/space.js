// Text-pattern attraction, explicit connections, and user-defined categories.
const stop=new Set('a an and are as at be been but by can for from has have i in into is it its me my of on or our that the their them these they this to use using was we were what when where which who will with you your app idea'.split(' '));
function terms(record){const out=new Map();for(const [text,weight] of [[record.title||'',3],[record.body||'',1]])for(let word of text.toLowerCase().match(/[\p{L}\p{N}]+/gu)||[]){if(word.length<3||stop.has(word))continue;if(word.length>5&&word.endsWith('ing'))word=word.slice(0,-3);else if(word.length>4&&word.endsWith('s'))word=word.slice(0,-1);out.set(word,(out.get(word)||0)+weight);}return out;}
export function affinities(records){const vectors=records.map(terms),df=new Map();for(const v of vectors)for(const word of v.keys())df.set(word,(df.get(word)||0)+1);const norms=vectors.map(v=>{let sum=0;for(const [word,count]of v){const x=(1+Math.log(count))*Math.log(1+records.length/(df.get(word)||1));v.set(word,x);sum+=x*x;}return Math.sqrt(sum)||1;});return records.map((a,i)=>records.map((b,j)=>{if(i===j)return 0;let dot=0;for(const[word,count]of vectors[i])dot+=count*(vectors[j].get(word)||0);let score=dot/(norms[i]*norms[j]);if(a.fields?.category&&a.fields.category===b.fields?.category)score=Math.max(score,.65);if(a.links?.includes(b.id)||b.links?.includes(a.id))score=Math.max(score,.85);return score;}));}
export class FloatingSpace {
 constructor(){this.nodes=new Map();this.records=[];this.weights=[];this.homes=new Map();this.sleeping=false;this.quietFrames=0;}
 sync(records,dimensions=new Map()){
  const previous=this.nodes,oldHomes=this.homes;this.records=records;this.weights=affinities(records);this.nodes=new Map();
  const parent=records.map((_,i)=>i);const root=i=>{while(parent[i]!==i){parent[i]=parent[parent[i]];i=parent[i];}return i;};
  for(let i=0;i<records.length;i++)for(let j=i+1;j<records.length;j++)if(this.weights[i][j]>=.28)parent[root(j)]=root(i);
  const groups=new Map();records.forEach((r,i)=>{const k=root(i);if(!groups.has(k))groups.set(k,[]);groups.get(k).push(r);});
  const placed=[],used=new Set();this.homes=new Map();
  for(const group of [...groups.values()].sort((a,b)=>b.length-a.length)){
   const existing=group.map(r=>oldHomes.get(r.id)).find(h=>h&&!used.has(h));let home=existing;
   const radius=Math.sqrt(group.length)*240+160;
   if(!home){let attempt=0;do{const angle=attempt*2.3999632297,d=attempt?650*Math.sqrt(attempt):0;home={x:Math.cos(angle)*d,y:Math.sin(angle)*d,radius};attempt++;}while(placed.some(h=>Math.hypot(home.x-h.x,home.y-h.y)<radius+h.radius+350)&&attempt<10000);}
   home.radius=radius;placed.push(home);used.add(home);
   group.forEach((r,i)=>{this.homes.set(r.id,home);const size=dimensions.get(r.id)||{w:266,h:260};const old=previous.get(r.id);const angle=i*2.3999632297,d=300*Math.sqrt(i);this.nodes.set(r.id,{...(old||{x:home.x+Math.cos(angle)*d,y:home.y+Math.sin(angle)*d,vx:0,vy:0}),...size});});
  }
  this.sleeping=false;this.quietFrames=0;
 }
 wake(){this.sleeping=false;this.quietFrames=0;}
 step(dt=1){
  if(this.sleeping)return false;dt=Math.min(2,Math.max(.1,dt));const r=this.records,n=r.length;if(!n)return false;const states=r.map(x=>this.nodes.get(x.id));
  for(let i=0;i<n;i++)for(let j=i+1;j<n;j++){
   const a=states[i],b=states[j];let dx=b.x+b.w/2-a.x-a.w/2,dy=b.y+b.h/2-a.y-a.h/2;
   if(Math.abs(dx)+Math.abs(dy)<.01){dx=Math.cos((i+j+1)*2.39996);dy=Math.sin((i+j+1)*2.39996);}
   const d=Math.max(1,Math.hypot(dx,dy)),weight=this.weights[i][j];let f=d<1400?-Math.min(.9,7000/(d*d)):0;
   if(weight>.10)f+=.00065*weight*(d-(390+(1-weight)*260));
   let fx=f*dx/d,fy=f*dy/d;const ox=(a.w+b.w)/2+90-Math.abs(dx),oy=(a.h+b.h)/2+90-Math.abs(dy);
   if(ox>0&&oy>0){if(ox<oy)fx-=Math.sign(dx)*Math.min(4,ox*.035);else fy-=Math.sign(dy)*Math.min(4,oy*.035);}
   a.vx+=fx*dt;a.vy+=fy*dt;b.vx-=fx*dt;b.vy-=fy*dt;
  }
  let maxSpeed=0;
  for(let i=0;i<n;i++){const s=states[i],home=this.homes.get(r[i].id);s.vx+=(home.x-s.x)*.00012*dt;s.vy+=(home.y-s.y)*.00012*dt;s.vx*=Math.pow(.85,dt);s.vy*=Math.pow(.85,dt);const speed=Math.hypot(s.vx,s.vy);maxSpeed=Math.max(maxSpeed,speed);if(speed>5){s.vx*=5/speed;s.vy*=5/speed;}s.x+=s.vx*dt;s.y+=s.vy*dt;}
  this.quietFrames=maxSpeed<.025?this.quietFrames+1:0;
  if(this.quietFrames>45){this.sleeping=true;for(const s of states){s.vx=0;s.vy=0;}}
  return true;
 }
}
