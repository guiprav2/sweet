<?php
declare(strict_types=1);

namespace Sweet;

final class Native
{
    private static ?\FFI $ffi = null;

    public static function ffi(): \FFI
    {
        if (self::$ffi === null) {
            if (!extension_loaded('FFI')) {
                throw new \RuntimeException('PHP FFI is required. Install/enable ext-ffi.');
            }
            $header = __DIR__ . '/sweet.h';
            $library = getenv('SWEET_PHP_LIBRARY') ?: dirname(__DIR__) . '/target/release/libsweet_php.so';
            self::$ffi = \FFI::cdef((string) file_get_contents($header), $library);
        }
        return self::$ffi;
    }

    public static function check(bool $ok, \FFI\CData $error): void
    {
        if (!$ok) {
            $message = \FFI::isNull($error) ? 'sweet-core operation failed' : \FFI::string($error);
            if (!\FFI::isNull($error)) self::ffi()->sweet_string_free($error);
            throw new \RuntimeException($message);
        }
        if (!\FFI::isNull($error)) {
            self::ffi()->sweet_string_free($error);
        }
    }
}

final class Document
{
    private \FFI\CData $native;
    private bool $closed = false;

    public function __construct()
    {
        $ffi = Native::ffi();
        $error = $ffi->new('char *');
        $this->native = $ffi->sweet_document_new(\FFI::addr($error));
        if (\FFI::isNull($this->native)) Native::check(false, $error);
    }

    public function __destruct() { $this->close(); }
    public function close(): void
    {
        if (!$this->closed) { Native::ffi()->sweet_document_free($this->native); $this->closed = true; }
    }

    public function body(): Node { return new Node($this, 0); }
    public function createElement(string $tag): Node
    {
        $ffi = Native::ffi(); $error = $ffi->new('char *');
        $id = $ffi->sweet_create_element($this->native, $tag, \FFI::addr($error));
        Native::check($id !== 4294967295, $error);
        return new Node($this, $id);
    }
    public function createText(string $text): Node
    {
        $ffi = Native::ffi(); $error = $ffi->new('char *');
        $id = $ffi->sweet_create_text($this->native, $text, \FFI::addr($error));
        Native::check($id !== 4294967295, $error);
        return new Node($this, $id);
    }
    public function renderPng(int $width, int $height): string
    {
        $ffi = Native::ffi(); $length = $ffi->new('size_t'); $error = $ffi->new('char *');
        $bytes = $ffi->sweet_render_png($this->native, $width, $height, \FFI::addr($length), \FFI::addr($error));
        if (\FFI::isNull($bytes)) Native::check(false, $error);
        $data = \FFI::string(\FFI::cast('char *', $bytes), $length->cdata);
        $ffi->sweet_bytes_free($bytes, $length->cdata);
        return $data;
    }
    public function savePng(string $path, int $width, int $height): void
    {
        if (file_put_contents($path, $this->renderPng($width, $height)) === false) throw new \RuntimeException("cannot write PNG: $path");
    }
    public function native(): \FFI\CData { return $this->native; }
}

final class Node
{
    public function __construct(private Document $document, public readonly int $id) {}
    public function appendChild(Node $child): Node
    {
        $ffi = Native::ffi(); $error = $ffi->new('char *');
        Native::check($ffi->sweet_append_child($this->document->native(), $this->id, $child->id, \FFI::addr($error)), $error);
        return $child;
    }
    public function setClass(string $className): self
    {
        $ffi = Native::ffi(); $error = $ffi->new('char *');
        Native::check($ffi->sweet_set_class($this->document->native(), $this->id, $className, \FFI::addr($error)), $error);
        return $this;
    }
    public function setText(string $text): self
    {
        $ffi = Native::ffi(); $error = $ffi->new('char *');
        Native::check($ffi->sweet_set_text($this->document->native(), $this->id, $text, \FFI::addr($error)), $error);
        return $this;
    }
    public function setAttribute(string $name, string $value): self
    {
        $ffi = Native::ffi(); $error = $ffi->new('char *');
        Native::check($ffi->sweet_set_attribute($this->document->native(), $this->id, $name, $value, \FFI::addr($error)), $error);
        return $this;
    }
    public function textContent(): string
    {
        $ffi = Native::ffi(); $error = $ffi->new('char *');
        $value = $ffi->sweet_text_content($this->document->native(), $this->id, \FFI::addr($error));
        if (\FFI::isNull($value)) Native::check(false, $error);
        $text = \FFI::string($value); $ffi->sweet_string_free($value); Native::check(true, $error);
        return $text;
    }
}
