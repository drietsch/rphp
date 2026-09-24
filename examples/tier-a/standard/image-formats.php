<?php
// getimagesizefromstring() over tiny images of every format php sniffs.
function show($label, $data) {
    $r = getimagesizefromstring($data);
    echo str_pad($label, 14), json_encode($r), "\n";
}
function le16($v) { return pack('v', $v); }
function le32($v) { return pack('V', $v); }

show('gif87', "GIF87a" . le16(10) . le16(20) . "\x00\x00\x00");
show('gif89 pal', "GIF89a" . le16(300) . le16(2) . "\xf7\x00\x00");
show('gif89 pal2', "GIF89a" . le16(1) . le16(1) . "\x81");
show('png', "\x89PNG\r\n\x1a\n" . pack('N', 13) . 'IHDR' . pack('NN', 640, 480) . "\x08\x06\0\0\0");
show('png 16', "\x89PNG\r\n\x1a\n" . pack('N', 13) . 'IHDR' . pack('NN', 1, 70000) . "\x10\x02\0\0\0");
show('bmp core', 'BM' . str_repeat("\0", 12) . le32(12) . le16(7) . le16(9) . le16(1) . le16(24) . str_repeat("\0", 4));
show('bmp info', 'BM' . str_repeat("\0", 12) . le32(40) . le32(33) . le32(44) . le16(1) . le16(32));
show('bmp neg h', 'BM' . str_repeat("\0", 12) . le32(40) . le32(5) . pack('l', -6) . le16(1) . le16(8));
show('bmp v4', 'BM' . str_repeat("\0", 12) . le32(108) . le32(2) . le32(3) . le16(1) . le16(16));
show('bmp v5', 'BM' . str_repeat("\0", 12) . le32(124) . le32(2) . le32(3) . le16(1) . le16(16));
show('bmp bad hdr', 'BM' . str_repeat("\0", 12) . le32(100) . le32(2) . le32(3) . le16(1) . le16(16));
show('psd', '8BPS' . "\0\x01" . str_repeat("\0", 6) . "\0\x03" . pack('NN', 77, 88) . "\0\x08\0\x03");
show('tiff II', "II\x2a\x00" . le32(8) . le16(3)
    . le16(0x100) . le16(3) . le32(1) . le32(7)
    . le16(0x101) . le16(4) . le32(1) . le32(9)
    . le16(0x102) . le16(3) . le32(1) . le32(8) . le32(0));
show('tiff MM', "MM\x00\x2a" . pack('N', 8) . pack('n', 2)
    . pack('nnNN', 0xA002, 4, 1, 1234) . pack('nnNN', 0xA003, 1, 1, 0x05000000) . pack('N', 0));
show('tiff no h', "II\x2a\x00" . le32(8) . le16(1) . le16(0x100) . le16(3) . le32(1) . le32(7) . le32(0));
show('tiff sshort', "II\x2a\x00" . le32(8) . le16(2)
    . le16(0x100) . le16(8) . le32(1) . le32(0xfffe) . le16(0x101) . le16(1) . le32(1) . le32(3) . le32(0));
show('tiff ifd 0', "II\x2a\x00" . le32(0) . "\x00\x00\x00\x00");
show('iff ilbm', 'FORM' . pack('N', 40) . 'ILBM' . 'ANNO' . pack('N', 3) . "abc\0" . 'BMHD' . pack('N', 20) . pack('nn', 320, 200) . "\0\0\0\0\x05" . str_repeat("\0", 11));
show('iff pbm', 'FORM' . pack('N', 40) . 'PBM ' . 'BMHD' . pack('N', 20) . pack('nn', 3, 4) . "\0\0\0\0\x08" . str_repeat("\0", 11));
show('iff bad bits', 'FORM' . pack('N', 40) . 'ILBM' . 'BMHD' . pack('N', 20) . pack('nn', 3, 4) . "\0\0\0\0\x28" . str_repeat("\0", 11));
show('iff 8svx', 'FORM' . pack('N', 40) . '8SVX' . str_repeat("\0", 20));
show('wbmp', "\x00\x00\x81\x00\x40" . str_repeat("\0", 20));
show('wbmp hdr', "\x00\x80\x80\x00\x05\x06");
show('wbmp big', "\x00\x00\x90\x01\x05" . str_repeat('.', 10));
show('wbmp zero', "\x00\x00\x00\x05");
show('xbm', "#define img_width 12\n#define img_height 3\nstatic unsigned char img_bits[] = {0};\n");
show('xbm height1', "/* x */\n#define h_height 5\n#define w_width 6\n");
show('xbm plain', "#define width 4\n#define height 2\nxxxxxxxx");
show('xbm crlf', "#define a_width 4\r\n#define a_height 2\r\n");
show('xbm none', "#define a_width 4\n#define a_depth 2\nxxxx");
show('ico', "\0\0\1\0\2\0" . "\x10\x10\0\0\1\0\x20\0" . str_repeat("\0", 8) . "\x20\x40\0\0\1\0\x08\0" . str_repeat("\0", 8));
show('ico 256', "\0\0\1\0\1\0" . "\0\0\0\0\1\0\x20\0" . str_repeat("\0", 8));
show('ico none', "\0\0\1\0\0\0" . str_repeat("\0", 16));
show('ico short', "\0\0\1\0\3\0" . "\x05\x06\0\0\1\0\x04\0" . str_repeat("\0", 8));
show('webp lossy', 'RIFF' . le32(30) . 'WEBP' . 'VP8 ' . le32(10) . "\0\0\0\x9d\x01\x2a" . le16(100) . le16(0xC032));
show('webp lossless', 'RIFF' . le32(30) . 'WEBP' . 'VP8L' . le32(10) . "\x2f" . pack('V', (49) | (29 << 14)) . "\0\0");
show('webp extended', 'RIFF' . le32(30) . 'WEBP' . 'VP8X' . le32(10) . "\x10\0\0\0" . "\xff\x0f\0" . "\x01\x02\x03");
show('webp bad', 'RIFF' . le32(30) . 'WEBP' . 'VP9 ' . str_repeat("\0", 14));
show('riff wave', 'RIFF' . le32(30) . 'WAVEfmt ' . str_repeat("\0", 14));
show('jpc', "\xff\x4f\xff\x51\x00\x29\x00\x00" . pack('NN', 17, 19) . str_repeat("\0", 24) . "\x00\x03" . "\x07\x01\x01\x0b\x01\x01\x07\x01\x01");
show('jpc 0 comp', "\xff\x4f\xff\x51\x00\x29\x00\x00" . pack('NN', 17, 19) . str_repeat("\0", 24) . "\x00\x00\x00");
show('jp2', "\x00\x00\x00\x0cjP  \x0d\x0a\x87\x0a" . pack('N', 20) . 'ftypjp2 ' . "\0\0\0\0jp2 " . pack('N', 0) . 'jp2c'
    . "\xff\x4f\xff\x51\x00\x29\x00\x00" . pack('NN', 5, 6) . str_repeat("\0", 24) . "\x00\x01\x0f\x01\x01");
$body = "\x50\x00" . str_repeat("\x00", 40);
// A frame rectangle of 16-bit fields: 0..8000 x 0..4000 twips.
$bits = sprintf('%05b', 16) . sprintf('%016b', 0) . sprintf('%016b', 8000) . sprintf('%016b', 0) . sprintf('%016b', 4000);
$bits = str_pad($bits, 256, '0');
$rect = '';
foreach (str_split($bits, 8) as $byte) {
    $rect .= chr(bindec($byte));
}
show('swf', 'FWS' . "\x06" . le32(100) . $rect);
show('swf short', 'FWS' . "\x06" . le32(100) . substr($rect, 0, 20));
$noise = '';
for ($i = 0; $i < 12; $i++) {
    $noise .= md5("n$i", true);
}
show('swc', 'CWS' . "\x06" . le32(100) . gzcompress($rect . $noise));
show('swc trunc', substr('CWS' . "\x06" . le32(100) . gzcompress($rect . $noise), 0, -5));
show('swc short', 'CWS' . "\x06" . le32(100) . gzcompress($rect));
show('swc bad', 'CWS' . "\x06" . le32(100) . str_repeat("\x55", 80));
