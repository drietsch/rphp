<?php
// SVG (IMAGETYPE_SVG): the root element's width/height, units, and the
// well-formedness the XML reader insists on.
$cases = [
    '<svg width="10" height="20"/>',
    '<svg width="10px" height="20px"/>',
    '<svg width="10em" height="2in"/>',
    '<svg width="10cm" height="5pc"/>',
    '<svg width="5PX" height="4MM"/>',
    '<svg width="10.7" height="20.2"/>',
    '<svg width="10%" height="5pt"/>',
    '<svg width=" 10 " height="-5"/>',
    '<svg width="" height="4"/>',
    '<svg viewBox="0 0 30 40"/>',
    '<svg width="10"/>',
    '<svg width="0" height="0"/>',
    '<svg width="010" height="4"/>',
    '<svg width="4294967297" height="4"/>',
    "<svg width='3' height='4'/>",
    '<svg width="&#51;" height="4"/>',
    '<!DOCTYPE svg [<!ENTITY w "7">]><svg width="&w;" height="4"/>',
    '<?xml version="1.0"?><!-- c --><svg xmlns="http://www.w3.org/2000/svg" width="3" height="4"></svg>',
    '<?xml version="1.0" encoding="ISO-8859-1"?><svg width="3" height="4"/>',
    '<!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd"><svg width="3" height="4"/>',
    '<svg:svg xmlns:svg="http://www.w3.org/2000/svg" width="3" height="4"/>',
    '<x:svg width="3" height="4"/>',
    '<SVG width="3" height="4"/>',
    '<svgx width="3" height="4"/>',
    '<html><svg width="3" height="4"/></html>',
    '   <svg width="3" height="4"/>',
    '<svg width="3" height="4">',
    '<svg width="3" height="4"><a',
    '<svg width="3" height="4"><a/><![CDATA[<<]]><?pi x?><!-- c -->&amp;&#65;</svg><!-- after --> ',
    '<svg width="3" height="4" xmlns:x="u"><x:a/><y:b/></svg>',
    '<svg width="3" height="4"><a></b></svg>',
    '<svg width="3" height="4"></svg><x/>',
    '<svg width="3" height="4">&nope;</svg>',
    '<svg width="3" height="4">a < b</svg>',
    '<svg width="3" width="5" height="4"/>',
    '<svg width="3" height="4" x="a<b"/>',
    '<svg width="3"height="4"/>',
    '<?xml?><svg width="3" height="4"/>',
];
foreach ($cases as $s) {
    echo $s, "\n  => ", json_encode(getimagesizefromstring($s)), "\n";
}
// Errors deep in the document only count once the reader has read them
// (it parses 512-byte chunks).
foreach ([100, 480, 490, 600, 3000] as $n) {
    $s = '<svg width="3" height="4">' . str_repeat('x', $n) . '</b>';
    echo $n, ': ', json_encode(getimagesizefromstring($s) !== false), "\n";
}
file_put_contents('image-svg.svg', '<svg width="3" height="4"/>');
echo json_encode(getimagesize('image-svg.svg')), "\n";
unlink('image-svg.svg');
