<?php
// getimagesize()/getimagesizefromstring() failure paths: short input,
// unknown data, corrupt signatures, missing files, bad arguments.
function t($label, $f) {
    echo "-- $label\n";
    try {
        var_dump($f());
    } catch (\Throwable $e) {
        echo get_class($e), ': ', $e->getMessage(), "\n";
    }
}
t('empty string', fn() => getimagesizefromstring(''));
t('one byte', fn() => getimagesizefromstring('G'));
t('GIF only', fn() => getimagesizefromstring('GIF'));
t('GIF89a', fn() => getimagesizefromstring('GIF89a'));
t('BM', fn() => getimagesizefromstring('BM'));
t('abcd', fn() => getimagesizefromstring('abcd'));
t('eleven', fn() => getimagesizefromstring('hello world'));
t('twelve', fn() => getimagesizefromstring('hello world!'));
t('text', fn() => getimagesizefromstring(str_repeat("plain text\n", 20)));
t('int', fn() => getimagesizefromstring(12345));
t('png ascii', fn() => getimagesizefromstring("\x89PNG\n\x1a\n\0\0\0\0IHDR"));
t('png sig only', fn() => getimagesizefromstring("\x89PNG\r\n\x1a\n"));
t('riff short', fn() => getimagesizefromstring('RIFFabc'));
t('jpc no siz', fn() => getimagesizefromstring("\xff\x4f\xff\x52\x00\x00"));
t('jp2 no codestream', fn() => getimagesizefromstring("\x00\x00\x00\x0cjP  \x0d\x0a\x87\x0a" . pack('N', 8) . 'abcd'));
t('jp2 xlbox', fn() => getimagesizefromstring("\x00\x00\x00\x0cjP  \x0d\x0a\x87\x0a" . pack('N', 1) . 'jp2h'));
t('info reset', function () {
    $info = 'x';
    $r = getimagesizefromstring('abc', $info);
    var_dump($info);
    return $r;
});
t('missing file', fn() => getimagesize('no-such-image.png'));
t('missing info', function () {
    $info = 42;
    $r = getimagesize('no-such-image.png', $info);
    var_dump($info);
    return $r;
});
t('empty path', fn() => getimagesize(''));
t('nul path', fn() => getimagesize("a\0b"));
t('array arg', fn() => getimagesize([]));
t('no args', fn() => getimagesizefromstring());
t('too many', fn() => getimagesize('a', $x, 3));

// From a file in the working directory, and through data:.
$gif = "GIF89a" . pack('vv', 3, 4) . "\x80\0\0";
file_put_contents('image-errors.gif', $gif);
t('file', fn() => getimagesize('image-errors.gif'));
t('file info', function () {
    $r = getimagesize('image-errors.gif', $info);
    var_dump($info);
    return $r;
});
unlink('image-errors.gif');
file_put_contents('image-errors.txt', 'ab');
t('short file', fn() => getimagesize('image-errors.txt'));
unlink('image-errors.txt');
