<?php
// Directory streams through a user wrapper: opendir/readdir/rewinddir/
// closedir, scandir, dir() and Directory.
class D {
    public $context;
    private $list = ['.', '..', 'b', 'a', 'c'];
    private $i = 0;
    function dir_opendir($p, $o) { echo "opendir $p $o ", gettype($this->context), "\n"; return !str_contains($p, 'no'); }
    function dir_readdir() { echo "readdir\n"; return $this->list[$this->i++] ?? false; }
    function dir_rewinddir() { echo "rewind\n"; $this->i = 0; return true; }
    function dir_closedir() { echo "closedir\n"; return true; }
}
stream_wrapper_register('dd', 'D');
$d = opendir('dd://top');
var_dump(get_resource_type($d));
var_dump(readdir($d), readdir($d), readdir($d));
rewinddir($d);
var_dump(readdir($d));
$m = stream_get_meta_data($d);
var_dump($m['wrapper_type'], $m['stream_type'], isset($m['uri']));
closedir($d);
var_dump(scandir('dd://top'));
var_dump(scandir('dd://top', SCANDIR_SORT_DESCENDING));
var_dump(scandir('dd://top', SCANDIR_SORT_NONE));
$dir = dir('dd://top');
var_dump($dir->read(), $dir->read());
$dir->rewind();
var_dump($dir->read());
$dir->close();
var_dump(opendir('dd://no'));
var_dump(scandir('dd://no'));
