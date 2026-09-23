<?php
// opendir/readdir/rewinddir/closedir, dir() and the Directory class.
$base = sys_get_temp_dir() . '/dirs-' . getmypid();
mkdir("$base/sub", 0777, true); touch("$base/b.txt"); touch("$base/a.txt");
chdir($base);
$h = opendir('.'); var_dump($h);
$e = []; while (($x = readdir($h)) !== false) $e[] = $x; sort($e); var_dump($e);
rewinddir($h); var_dump(readdir($h)); var_dump(readdir());
closedir($h); var_dump(get_resource_type($h));
try { readdir($h); } catch (TypeError $e) { echo $e->getMessage(), "\n"; }
try { readdir(fopen('php://memory', 'r')); } catch (TypeError $e) { echo $e->getMessage(), "\n"; }
try { readdir('x'); } catch (TypeError $e) { echo $e->getMessage(), "\n"; }
try { closedir(); } catch (TypeError $e) { echo $e->getMessage(), "\n"; }
var_dump(@opendir('/nonexistent')); opendir('/nonexistent'); opendir("a.txt");
$d = dir('.'); var_dump($d); var_dump($d->read(), $d->path); $d->rewind(); var_dump($d->read()); $d->close();
try { $d->read(); } catch (Error $e) { echo get_class($e), ': ', $e->getMessage(), "\n"; }
try { new Directory; } catch (Error $e) { echo get_class($e), ': ', $e->getMessage(), "\n"; }
var_dump(dir('/nonexistent'));
$names = []; foreach (glob("$base/*") as $f) { $names[] = basename($f); } var_dump($names);
unlink("$base/a.txt"); unlink("$base/b.txt"); rmdir("$base/sub"); chdir('/'); rmdir($base);
