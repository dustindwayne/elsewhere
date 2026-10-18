// Serialize remote changes so a batch remains readable.
export class ChangeSequence {
 constructor(){this.queue=[];this.running=false;}
 add(change){this.queue.push(change);if(!this.running)this.run();}
 async run(){this.running=true;try{while(this.queue.length){try{await this.queue.shift()();}catch(error){console.error('Visual transition failed',error);}}}finally{this.running=false;}}
}
export async function fade(element,frames,duration=260){if(!element.isConnected)return;if(matchMedia('(prefers-reduced-motion: reduce)').matches)return;try{await element.animate(frames,{duration,easing:'ease-in-out'}).finished;}catch{}}
