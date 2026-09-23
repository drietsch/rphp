<?php
// The bucket API a user filter works with: brigades, StreamBucket objects,
// append/prepend, new buckets, and the argument checks.

class inspector extends php_user_filter {
    public function filter($in, $out, &$consumed, $closing): int {
        if ($closing) {
            return PSFS_PASS_ON;
        }
        $b = stream_bucket_make_writeable($in);
        if ($b === null) {
            echo "no buckets\n";
            return PSFS_FEED_ME;
        }
        echo get_resource_type($in), " / ", get_resource_type($out), "\n";
        echo get_class($b), " ", get_resource_type($b->bucket), " ", $b->data, " ", $b->datalen, " ", $b->dataLength, "\n";
        var_dump(stream_bucket_make_writeable($in));
        $consumed += $b->datalen;

        $new = stream_bucket_new($this->stream, "<new>");
        echo get_resource_type($new->bucket), " ", $new->data, " ", $new->datalen, "\n";

        // Out goes: prepended first, then the rest in order.
        stream_bucket_append($out, $new);
        $b->data = strrev($b->data);
        stream_bucket_prepend($out, $b);
        stream_bucket_append($out, stream_bucket_new($this->stream, "<tail>"));
        // The same bucket twice is still one bucket.
        stream_bucket_append($out, $new);

        foreach ([
            fn() => stream_bucket_append($out, new \stdClass),
            fn() => stream_bucket_append($this->stream, $new),
            fn() => stream_bucket_new(42, "x"),
            fn() => stream_bucket_make_writeable($this->stream),
            fn() => stream_bucket_make_writeable("x"),
        ] as $bad) {
            try {
                $bad();
            } catch (\TypeError $e) {
                echo get_class($e), ": ", $e->getMessage(), "\n";
            }
        }
        return PSFS_PASS_ON;
    }
}
stream_filter_register('inspector', 'inspector');
$m = fopen('php://memory', 'w+');
stream_filter_append($m, 'inspector', STREAM_FILTER_WRITE);
var_dump(fwrite($m, "abc"));
rewind($m);
var_dump(stream_get_contents($m));

echo "--- a bucket made outside a filter ---\n";
$b = stream_bucket_new($m, "free standing");
print_r($b);
fclose($m);

echo "--- the base class's own methods ---\n";
$f = new php_user_filter();
var_dump($f);
var_dump($f->onCreate(), $f->onClose());
try {
    $f->filter(1, 2, $c, false);
} catch (\TypeError $e) {
    echo $e->getMessage(), "\n";
}

echo "--- constants ---\n";
foreach (['PSFS_PASS_ON', 'PSFS_FEED_ME', 'PSFS_ERR_FATAL', 'PSFS_FLAG_NORMAL', 'PSFS_FLAG_FLUSH_INC', 'PSFS_FLAG_FLUSH_CLOSE', 'STREAM_FILTER_READ', 'STREAM_FILTER_WRITE', 'STREAM_FILTER_ALL'] as $c) {
    echo $c, " = ", constant($c), "\n";
}
