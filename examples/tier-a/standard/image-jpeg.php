<?php
// JPEG: SOF variants, APPn markers into $image_info, padding and junk.
function seg($marker, $payload) { return "\xff" . chr($marker) . pack('n', strlen($payload) + 2) . $payload; }
function sof($marker, $w, $h, $bits = 8, $ch = 3) { return seg($marker, chr($bits) . pack('nn', $h, $w) . chr($ch) . str_repeat("\x01\x11\x00", $ch)); }
function show($label, $data, $withInfo = true) {
    echo "-- $label\n";
    if ($withInfo) {
        $info = 'untouched';
        $r = getimagesizefromstring($data, $info);
        echo json_encode($r), "\n";
        echo json_encode(array_map('bin2hex', $info)), "\n";
    } else {
        echo json_encode(getimagesizefromstring($data)), "\n";
    }
}
$soi = "\xff\xd8";
$eoi = "\xff\xd9";
show('baseline', $soi . seg(0xE0, "JFIF\0\x01\x02\0\0\x01\0\x01\0\0") . sof(0xC0, 64, 32) . seg(0xDA, "\x01\x01\x00\x00\x3f\x00") . $eoi);
show('progressive', $soi . sof(0xC2, 5, 7, 12, 1) . $eoi);
show('lossless', $soi . sof(0xC3, 9, 9, 16, 4) . $eoi);
show('sof15', $soi . sof(0xCF, 3, 4) . $eoi);
show('dht first', $soi . seg(0xC4, str_repeat("\0", 17)) . seg(0xCC, "\0\0") . sof(0xC1, 11, 12) . $eoi);
show('apps', $soi . seg(0xE1, "Exif\0\0abc") . seg(0xE2, "ICC") . seg(0xED, "Photoshop 3.0\0") . seg(0xE1, "second") . seg(0xEF, '') . sof(0xC0, 2, 2) . seg(0xE3, 'late') . $eoi);
show('apps no info', $soi . seg(0xE1, "Exif\0\0abc") . sof(0xC0, 2, 2) . $eoi, false);
show('comment', $soi . seg(0xFE, 'a comment') . sof(0xC0, 20, 10) . $eoi);
show('padding ff', $soi . "\xff\xff\xff" . substr(sof(0xC0, 8, 9), 1) . $eoi);
show('junk', $soi . "junk!" . sof(0xC0, 8, 9) . $eoi);
show('no sof', $soi . seg(0xE0, 'JFIF') . "\xff\xda\x00\x02" . $eoi);
show('eoi', $soi . $eoi);
show('bare soi', "\xff\xd8\xff");
show('trunc app', $soi . "\xff\xe1\x00\x40abc");
show('short length', $soi . "\xff\xe1\x00\x01" . sof(0xC0, 1, 1));
show('two sof', $soi . sof(0xC0, 1, 2) . sof(0xC2, 3, 4) . seg(0xE4, 'x') . $eoi);
show('trunc sof', $soi . "\xff\xc0\x00\x11\x08\x00");
show('sof len 7', $soi . "\xff\xc0\x00\x07\x08\x00\x05\x00\x06\x03" . seg(0xE0, 'x'));
