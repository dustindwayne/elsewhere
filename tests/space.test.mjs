import test from 'node:test';
import assert from 'node:assert/strict';
import {FloatingSpace,affinities} from '../web/space.js';
import {ChangeSequence} from '../web/transitions.js';
const record=(id,title,body,extra={})=>({id,title,body,fields:{},links:[],...extra});
test('shared patterns attract without merging unrelated regions',()=>{
 const records=[record('a','Forest ecology','Trees roots soil canopy'),record('b','Forest canopy','Trees roots soil'),record('c','Orbital mechanics','Satellite trajectory velocity')];
 const scores=affinities(records);assert(scores[0][1]>scores[0][2]);const field=new FloatingSpace();field.sync(records);assert.equal(field.homes.get('a'),field.homes.get('b'));assert.notEqual(field.homes.get('a'),field.homes.get('c'));
 for(let i=0;i<12000&&!field.sleeping;i++)field.step();assert(field.sleeping,'The layout must settle');
 const point=id=>field.nodes.get(id),distance=(a,b)=>Math.hypot(a.x-b.x,a.y-b.y);assert(distance(point('a'),point('b'))<distance(point('a'),point('c')));
 for(let i=0;i<records.length;i++)for(let j=i+1;j<records.length;j++){const a=point(records[i].id),b=point(records[j].id);assert(Math.abs(a.x-b.x)>=(a.w+b.w)/2||Math.abs(a.y-b.y)>=(a.h+b.h)/2);}
 const before=JSON.stringify([...field.nodes]);for(let i=0;i<200;i++)field.step();assert.equal(JSON.stringify([...field.nodes]),before);
 const home=field.homes.get('a');field.sync([...records,record('d','Forest soil','Tree roots canopy')]);assert.equal(field.homes.get('a'),home);assert.equal(field.homes.get('d'),home);
});
test('connections and arbitrary category names form stable groups',()=>{
 const records=[record('a','Alpha','', {fields:{category:'Custom group'}}),record('b','Beta','',{fields:{category:'Custom group'}}),record('c','Gamma','',{links:['a']})];const field=new FloatingSpace();field.sync(records);assert.equal(field.homes.get('a'),field.homes.get('b'));assert.equal(field.homes.get('a'),field.homes.get('c'));
});
test('remote visual changes run one at a time in arrival order',async()=>{const sequence=new ChangeSequence(),events=[];let release;const gate=new Promise(resolve=>release=resolve);sequence.add(async()=>{events.push('start-a');await gate;events.push('end-a');});sequence.add(async()=>{events.push('start-b');events.push('end-b');});assert.deepEqual(events,['start-a']);release();await new Promise(resolve=>setTimeout(resolve,0));assert.deepEqual(events,['start-a','end-a','start-b','end-b']);});
