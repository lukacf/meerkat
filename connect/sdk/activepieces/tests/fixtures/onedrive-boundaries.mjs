// ECMAScript WhiteSpace and LineTerminator characters consumed by String.trim.
// ASCII controls still fail the profile's separate scalar-text contract.
export const trimCharacters = ['\u0009','\u000a','\u000b','\u000c','\u000d','\u0020','\u00a0','\u1680',
 ...Array.from({length:11},(_,i)=>String.fromCodePoint(0x2000+i)),'\u2028','\u2029','\u202f','\u205f','\u3000','\ufeff'];
export function oneDrivePathBoundaries() {
 const paths=new Set(), alphabet=['a','.','/',' ','\ufeff','\u0085'];
 function strings(prefix,remaining){paths.add(prefix);if(remaining)for(const atom of alphabet)strings(prefix+atom,remaining-1)}
 strings('',6);
 const outer=['',...trimCharacters,'\u0085','\u180e','\u200b',' \ufeff','\u00a0\u3000'];
 const cores=['','a','items','items/a.txt','reports/items/a.txt','.','..','...','a/b','a//b','a///b','a/./b','a/../b',
  './a','../a','a/.','a/..',' a ',' . ',' .. ',' ','\ufeff','a// /b','a/ /b','💡','å#?%'];
 const slashes=[['',''],['/','/'],['//','///'],['///','//'],['','///'],['///','']];
 for(const before of outer)for(const after of outer)for(const [leading,trailing]of slashes)for(const core of cores)paths.add(before+leading+core+trailing+after);
 for(const atom of ['a','💡','\u0085','\u200b'])for(const size of [255,256,257])paths.add(atom.repeat(size));
 for(const bad of ['\u0000','\u001f','\u007f','\ud800','\udfff'])for(const base of ['','a','//a//','a/b']){
  paths.add(bad+base);paths.add(base+bad);paths.add('a/'+bad+'/b');
 }
 return [...paths];
}
