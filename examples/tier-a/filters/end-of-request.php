<?php
// A stream still open when the request ends is closed then: its write
// chain gets the closing call — with $this->stream already null — and
// every filter its onClose(), after the shutdown functions and the output.

class last_word extends php_user_filter {
    public function filter($in, $out, &$consumed, $closing): int {
        while ($b = stream_bucket_make_writeable($in)) {
            $consumed += $b->datalen;
            stream_bucket_append($out, $b);
        }
        echo "filter(", $this->filtername, ") closing=", var_export($closing, true),
            " stream=", get_debug_type($this->stream), "\n";
        return PSFS_PASS_ON;
    }
    public function onClose(): void {
        echo "onClose(", $this->filtername, ")\n";
    }
}
stream_filter_register('last.*', 'last_word');

$keep = fopen('php://memory', 'w+');
stream_filter_append($keep, 'last.write', STREAM_FILTER_WRITE);
fwrite($keep, "data");

register_shutdown_function(function () {
    echo "shutdown function\n";
});
echo "end of script\n";
