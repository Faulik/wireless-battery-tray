// Run: node cloudflare/negotiate.test.mjs
import { negotiate } from "./worker.js";
const cases = [
 [undefined,"html"],["text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8","html"],
 ["*/*","html"],["text/markdown","markdown"],["text/markdown, text/html","markdown"],
 ["text/html, text/markdown;q=0.5","html"],["text/markdown, text/html;q=0.9, */*;q=0.1","markdown"],
 ["image/png",null],["text/plain",null],["text/*","html"],["text/markdown;q=0",null]];
let bad=0; for (const [a,e] of cases){const g=negotiate(a); if(g!==e){bad++;console.log("FAIL",a,g,e)}}
console.log(bad ? `${bad} failed` : `all ${cases.length} ok`);
if (bad) process.exit(1);
