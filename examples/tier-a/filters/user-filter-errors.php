<?php
// The ways a user filter goes wrong, and what php says about each.

echo "--- stream_filter_register() arguments ---\n";
foreach ([['', 'x'], ['x', ''], ['', '']] as [$name, $class]) {
    try {
        stream_filter_register($name, $class);
    } catch (\ValueError $e) {
        echo $e->getMessage(), "\n";
    }
}

echo "--- a class that does not exist ---\n";
var_dump(stream_filter_register('ghost', 'NoSuchClass'));
$m = fopen('php://memory', 'w+');
var_dump(stream_filter_append($m, 'ghost'));

echo "--- onCreate() says no ---\n";
class refuses extends php_user_filter {
    public function onCreate(): bool {
        echo "onCreate(", $this->filtername, ")\n";
        return false;
    }
}
stream_filter_register('refuses', 'refuses');
var_dump(stream_filter_append($m, 'refuses'));
fwrite($m, "still plain");
rewind($m);
var_dump(stream_get_contents($m));
fclose($m);

echo "--- names nobody registered ---\n";
$m = fopen('php://memory', 'w+');
var_dump(stream_filter_append($m, 'no.such.filter'));
var_dump(stream_filter_append($m, ''));
var_dump(stream_filter_append($m, 'String.Rot13'));
var_dump(stream_filter_append($m, 'convert.nothing'));
fclose($m);

echo "--- the base class fails every call ---\n";
class base_only extends php_user_filter {}
stream_filter_register('base_only', 'base_only');
$m = fopen('php://memory', 'w+');
stream_filter_append($m, 'base_only', STREAM_FILTER_WRITE);
var_dump(fwrite($m, "abc"));
fclose($m);

echo "--- PSFS_ERR_FATAL, with and without taking the buckets ---\n";
class fails extends php_user_filter {
    public function filter($in, $out, &$consumed, $closing): int {
        if ($this->params === 'take') {
            while ($b = stream_bucket_make_writeable($in)) {
                $consumed += $b->datalen;
            }
        }
        return PSFS_ERR_FATAL;
    }
}
stream_filter_register('fails', 'fails');
foreach (['leave', 'take'] as $how) {
    echo "$how, write:\n";
    $m = fopen('php://memory', 'w+');
    $f = stream_filter_append($m, 'fails', STREAM_FILTER_WRITE, $how);
    var_dump(fwrite($m, "abc"));
    var_dump(stream_filter_remove($f));
    fclose($m);
    echo "$how, read:\n";
    $m = fopen('php://memory', 'w+');
    fwrite($m, "abc");
    rewind($m);
    stream_filter_append($m, 'fails', STREAM_FILTER_READ, $how);
    var_dump(fread($m, 10));
    var_dump(stream_get_contents($m));
    fclose($m);
}

echo "--- PSFS_FEED_ME holds everything back ---\n";
class hoards extends php_user_filter {
    private $held = '';
    public function filter($in, $out, &$consumed, $closing): int {
        while ($b = stream_bucket_make_writeable($in)) {
            $this->held .= $b->data;
            $consumed += $b->datalen;
        }
        if (!$closing) {
            return PSFS_FEED_ME;
        }
        stream_bucket_append($out, stream_bucket_new($this->stream, "[" . $this->held . "]"));
        return PSFS_PASS_ON;
    }
}
stream_filter_register('hoards', 'hoards');
$fn = sys_get_temp_dir() . '/user-filter-errors.txt';
$h = fopen($fn, 'w');
stream_filter_append($h, 'hoards');
var_dump(fwrite($h, "a"), fwrite($h, "b"), fflush($h));
var_dump(filesize($fn));
fclose($h);
var_dump(file_get_contents($fn));
unlink($fn);

echo "--- an exception out of filter() ---\n";
class throws extends php_user_filter {
    public function filter($in, $out, &$consumed, $closing): int {
        throw new \RuntimeException("filter blew up");
    }
}
stream_filter_register('throws', 'throws');
$m = fopen('php://memory', 'w+');
$f = stream_filter_append($m, 'throws', STREAM_FILTER_WRITE);
try {
    fwrite($m, "abc");
} catch (\RuntimeException $e) {
    echo get_class($e), ": ", $e->getMessage(), "\n";
}
try {
    stream_filter_remove($f);
} catch (\RuntimeException $e) {
    echo get_class($e), ": ", $e->getMessage(), "\n";
}
try {
    var_dump(fclose($m));
} catch (\RuntimeException $e) {
    echo get_class($e), ": ", $e->getMessage(), "\n";
}
var_dump(get_resource_type($m));

echo "--- a filter may not close its own stream ---\n";
class closer extends php_user_filter {
    public function filter($in, $out, &$consumed, $closing): int {
        while ($b = stream_bucket_make_writeable($in)) {
            $consumed += $b->datalen;
            stream_bucket_append($out, $b);
        }
        if (!$closing) {
            var_dump(fclose($this->stream));
        }
        return PSFS_PASS_ON;
    }
}
stream_filter_register('closer', 'closer');
$m = fopen('php://memory', 'w+');
stream_filter_append($m, 'closer', STREAM_FILTER_WRITE);
var_dump(fwrite($m, "abc"));
var_dump(get_resource_type($m));
fclose($m);

echo "--- stream_filter_append() and _remove() arguments ---\n";
try {
    stream_filter_append("nope", 'string.rot13');
} catch (\TypeError $e) {
    echo $e->getMessage(), "\n";
}
$m = fopen('php://memory', 'w+');
$f = stream_filter_append($m, 'string.rot13');
fclose($m);
try {
    stream_filter_append($m, 'string.rot13');
} catch (\TypeError $e) {
    echo $e->getMessage(), "\n";
}
var_dump(get_resource_type($f));
try {
    stream_filter_remove($f);
} catch (\TypeError $e) {
    echo $e->getMessage(), "\n";
}
try {
    stream_filter_remove(STDOUT);
} catch (\TypeError $e) {
    echo $e->getMessage(), "\n";
}
try {
    stream_filter_remove(42);
} catch (\TypeError $e) {
    echo $e->getMessage(), "\n";
}
