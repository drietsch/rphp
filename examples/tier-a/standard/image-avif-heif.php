<?php
// AVIF and HEIF: ISOBMFF boxes, the primary item's ispe/pixi/av1C.
function box($t, $d) { return pack('N', 8 + strlen($d)) . $t . $d; }
function fbox($t, $d, $v = 0, $f = 0) { return box($t, pack('N', ($v << 24) | $f) . $d); }
function ftyp($major, ...$compat) { return box('ftyp', $major . "\0\0\0\0" . implode('', $compat)); }
function ispe($w, $h) { return fbox('ispe', pack('NN', $w, $h)); }
function pixi(...$b) { return fbox('pixi', chr(count($b)) . implode('', array_map('chr', $b))); }
function av1c($b2) { return box('av1C', "\x81\x20" . chr($b2) . "\0"); }
function auxc() { return fbox('auxC', "urn:mpeg:mpegB:cicp:systems:auxiliary:alpha\0"); }
function pitm($id) { return fbox('pitm', pack('n', $id)); }
function infe($id, $type) { return fbox('infe', pack('nn', $id, 0) . $type . "\0", 2); }
function iinf(...$e) { return fbox('iinf', pack('n', count($e)) . implode('', $e)); }
function ipma(...$items) {
    $d = pack('N', count($items));
    foreach ($items as [$id, $props]) {
        $d .= pack('n', $id) . chr(count($props)) . implode('', array_map('chr', $props));
    }
    return fbox('ipma', $d);
}
function iprp($props, ...$assoc) { return box('iprp', box('ipco', implode('', $props)) . ipma(...$assoc)); }
function meta(...$c) { return fbox('meta', fbox('hdlr', "\0\0\0\0pict" . str_repeat("\0", 13)) . implode('', $c)); }
function show($label, $data) { echo str_pad($label, 20), json_encode(getimagesizefromstring($data)), "\n"; }

$item = iinf(infe(1, 'av01'));
$std = meta(pitm(1), $item, iprp([ispe(8, 6), pixi(8, 8, 8), av1c(0x0c)], [1, [1, 0x82, 3]]));
show('avif', ftyp('avif', 'avif', 'mif1', 'miaf') . $std . box('mdat', 'xx'));
show('avis major', ftyp('avis', 'avis') . $std);
show('avif compat', ftyp('mif1', 'mif1', 'avif') . $std);
show('heic', ftyp('heic', 'mif1', 'heic') . $std);
show('heix', ftyp('heix') . $std);
show('mif1', ftyp('mif1', 'mif1') . $std);
show('hevc', ftyp('hevc') . $std);
show('compat heic only', ftyp('abcd', 'heic') . $std);
show('av1C 10-bit', ftyp('avif', 'avif') . meta(pitm(1), $item, iprp([ispe(3, 4), av1c(0x40)], [1, [1, 2]])));
show('av1C 12-bit mono', ftyp('avif', 'avif') . meta(pitm(1), $item, iprp([ispe(3, 4), av1c(0x70)], [1, [1, 2]])));
show('av1C bad depth', ftyp('avif', 'avif') . meta(pitm(1), $item, iprp([ispe(3, 4), av1c(0x20)], [1, [1, 2]])));
show('alpha', ftyp('avif', 'avif') . meta(pitm(1), iinf(infe(1, 'av01'), infe(2, 'av01')),
    iprp([ispe(5, 5), pixi(8, 8, 8), auxc()], [1, [1, 2]], [2, [1, 3]])));
show('pixi 10', ftyp('avif', 'avif') . meta(pitm(1), $item, iprp([ispe(1, 2), pixi(10, 10, 10)], [1, [1, 2]])));
show('pixi mixed', ftyp('avif', 'avif') . meta(pitm(1), $item, iprp([ispe(1, 2), pixi(8, 10)], [1, [1, 2]])));
show('ispe zero', ftyp('avif', 'avif') . meta(pitm(1), $item, iprp([ispe(0, 2), pixi(8)], [1, [1, 2]])));
show('no pitm', ftyp('avif', 'avif') . meta($item, iprp([ispe(1, 2), pixi(8)], [1, [1, 2]])));
show('no iinf', ftyp('avif', 'avif') . meta(pitm(1), iprp([ispe(1, 2), pixi(8)], [1, [1, 2]])));
show('no pixi', ftyp('avif', 'avif') . meta(pitm(1), $item, iprp([ispe(1, 2)], [1, [1]])));
show('other item', ftyp('avif', 'avif') . meta(pitm(2), $item, iprp([ispe(1, 2), pixi(8)], [1, [1, 2]])));
show('grid', ftyp('avif', 'avif') . meta(pitm(1), iinf(infe(1, 'grid'), infe(2, 'av01')),
    fbox('iref', box('dimg', pack('nnn', 1, 1, 2))), iprp([ispe(40, 30), pixi(8, 8, 8)], [1, [1]], [2, [2]])));
show('no meta', ftyp('avif', 'avif') . box('free', 'abc'));
show('ftyp only', ftyp('avif', 'avif'));
show('short box', ftyp('avif', 'avif') . "\0\0\0\x40meta");
show('size 0 box', ftyp('avif', 'avif') . "\0\0\0\0meta\0\0\0\0");
