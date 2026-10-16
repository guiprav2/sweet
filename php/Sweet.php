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
    private bool $isDisposed = false;
    private array $listeners = [];
    private array $windows = [];

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
        if (!$this->isDisposed) { Native::ffi()->sweet_document_free($this->native); $this->isDisposed = true; }
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
    public function setText(int $id, string $text): void
    {
        $ffi = Native::ffi(); $error = $ffi->new('char *');
        Native::check($ffi->sweet_set_text($this->native, $id, $text, \FFI::addr($error)), $error);
    }
    public function setImageEncoded(int $id, string $bytes): void
    {
        $ffi = Native::ffi(); $error = $ffi->new('char *');
        $buffer = $ffi->new('uint8_t[' . max(1, strlen($bytes)) . ']');
        if ($bytes !== '') \FFI::memcpy($buffer, $bytes, strlen($bytes));
        $ok = $ffi->sweet_set_image_encoded($this->native, $id, $buffer, strlen($bytes), \FFI::addr($error));
        Native::check($ok, $error);
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
    public function addListener(int $id, string $event, callable $callback): void { $this->listeners[$id][$event][] = $callback; }
    public function dispatch(int $id, string $event, SweetEvent $detail): void { foreach ($this->listeners[$id][$event] ?? [] as $callback) $callback($detail); }
    public function dark(): bool { return Native::ffi()->sweet_dark($this->native); }
    public function setDark(bool $dark): void { Native::ffi()->sweet_set_dark($this->native, $dark); }
    public function createComment(string $text = ''): Node { return new Node($this, Native::ffi()->sweet_create_comment($this->native, $text)); }
    public function window(array $options = []): Window { return new Window($this, $options); }
    public function registerWindow(Window $window): void { $this->windows[spl_object_id($window)] = $window; }
    public function unregisterWindow(Window $window): void { unset($this->windows[spl_object_id($window)]); }
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
        $this->document->setText($this->id, $text);
        return $this;
    }
    public function setAttribute(string $name, string $value): self
    {
        $ffi = Native::ffi(); $error = $ffi->new('char *');
        Native::check($ffi->sweet_set_attribute($this->document->native(), $this->id, $name, $value, \FFI::addr($error)), $error);
        return $this;
    }
    public function setImageSource(string $path): self
    {
        if (strtolower($this->tagName()) !== 'img') throw new \LogicException('setImageSource() only applies to img elements');
        $resolved = str_starts_with($path, '/') || preg_match('/^[A-Za-z]:[\\\\\/]/', $path)
            ? $path
            : getcwd() . DIRECTORY_SEPARATOR . $path;
        $bytes = @file_get_contents($resolved);
        if ($bytes === false) throw new \RuntimeException("cannot read image source from process CWD: $path");
        $this->document->setImageEncoded($this->id, $bytes);
        return $this->setAttribute('src', $path);
    }
    public function removeAttribute(string $name): self
    {
        $ffi = Native::ffi(); $error = $ffi->new('char *');
        Native::check($ffi->sweet_remove_attribute($this->document->native(), $this->id, $name, \FFI::addr($error)), $error); return $this;
    }
    public function attribute(string $name): ?string
    {
        $ffi = Native::ffi(); $error = $ffi->new('char *');
        $value = $ffi->sweet_get_attribute($this->document->native(), $this->id, $name, \FFI::addr($error));
        Native::check(\FFI::isNull($error), $error);
        if (\FFI::isNull($value)) return null;
        $text = \FFI::string($value); $ffi->sweet_string_free($value); return $text;
    }
    public function className(): string
    {
        $ffi = Native::ffi(); $error = $ffi->new('char *');
        $value = $ffi->sweet_get_class($this->document->native(), $this->id, \FFI::addr($error));
        if (\FFI::isNull($value)) Native::check(false, $error);
        $text = \FFI::string($value); $ffi->sweet_string_free($value); Native::check(true, $error); return $text;
    }
    public function setClassName(string $value): self { return $this->setClass($value); }
    public function tagName(): string
    {
        $ffi = Native::ffi(); $error = $ffi->new('char *');
        $value = $ffi->sweet_tag_name($this->document->native(), $this->id, \FFI::addr($error));
        if (\FFI::isNull($value)) Native::check(false, $error);
        $text = \FFI::string($value); $ffi->sweet_string_free($value); Native::check(true, $error); return $text;
    }
    public function nodeType(): int
    {
        $ffi = Native::ffi(); $error = $ffi->new('char *');
        $type = $ffi->sweet_node_type($this->document->native(), $this->id, \FFI::addr($error));
        Native::check($type !== 0, $error); return $type;
    }
    public function children(): array
    {
        $ffi = Native::ffi(); $error = $ffi->new('char *');
        $count = $ffi->sweet_child_count($this->document->native(), $this->id, \FFI::addr($error));
        Native::check($count !== PHP_INT_MAX, $error);
        $result = [];
        for ($i = 0; $i < $count; $i++) {
            $error = $ffi->new('char *'); $id = $ffi->sweet_child_at($this->document->native(), $this->id, $i, \FFI::addr($error));
            Native::check($id !== 4294967295, $error); $result[] = new self($this->document, $id);
        }
        return $result;
    }
    public function parent(): ?self
    {
        $id = Native::ffi()->sweet_parent($this->document->native(), $this->id);
        return $id === 4294967295 ? null : new self($this->document, $id);
    }
    public function insertBefore(Node $child, ?Node $before = null): self
    {
        $ffi = Native::ffi(); $error = $ffi->new('char *');
        $ok = $ffi->sweet_insert_before($this->document->native(), $this->id, $child->id, $before?->id ?? 4294967295, \FFI::addr($error));
        Native::check($ok, $error); return $this;
    }
    public function removeChild(Node $child): self
    {
        $ffi = Native::ffi(); $error = $ffi->new('char *');
        Native::check($ffi->sweet_remove_child($this->document->native(), $this->id, $child->id, \FFI::addr($error)), $error); return $this;
    }
    public function remove(): void { $parent = $this->parent(); if ($parent) $parent->removeChild($this); }
    public function setTextContent(string $text): self { $this->document->setTextContent($this->id, $text); return $this; }
    public function classList(): ClassList { return new ClassList($this); }
    public function on(string $event, callable $callback): self
    {
        $this->document->addListener($this->id, $event, $callback); return $this;
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

final class ClassList
{
    public function __construct(private Node $node) {}
    public function add(string ...$classes): void
    {
        $current = preg_split('/\s+/', trim($this->node->className()), -1, PREG_SPLIT_NO_EMPTY) ?: [];
        foreach ($classes as $class) if (!in_array($class, $current, true)) $current[] = $class;
        $this->node->setClassName(implode(' ', $current));
    }
    public function remove(string ...$classes): void
    {
        $remove = array_fill_keys($classes, true);
        $current = preg_split('/\s+/', trim($this->node->className()), -1, PREG_SPLIT_NO_EMPTY) ?: [];
        $this->node->setClassName(implode(' ', array_values(array_filter($current, fn($class) => !isset($remove[$class])))));
    }
    public function contains(string $class): bool { return in_array($class, preg_split('/\s+/', trim($this->node->className()), -1, PREG_SPLIT_NO_EMPTY) ?: [], true); }
}

final class Window
{
    private \FFI\CData $native;
    private bool $closed = false;
    private bool $resizable = true;
    private string $resizeAxes = 'both';
    private string $safeArea = 'none';
    private array $listeners = [];
    public function __construct(private Document $document, array $options = [])
    {
        $ffi = Native::ffi(); $error = $ffi->new('char *');
        $this->resizeAxes = (string)($options['resizeAxes'] ?? 'both');
        [$horizontal, $vertical] = self::parseResizeAxes($this->resizeAxes);
        $this->safeArea = (string)($options['safeArea'] ?? 'none');
        self::parseSafeArea($this->safeArea);
        $this->native = $ffi->sweet_window_new($document->native(), (string)($options['title'] ?? 'sweet'), max(1, (int)($options['width'] ?? 960)), max(1, (int)($options['height'] ?? 600)), (bool)($options['dragAnywhere'] ?? false), $horizontal, $vertical, \FFI::addr($error));
        if (\FFI::isNull($this->native)) Native::check(false, $error);
        $document->registerWindow($this);
        if (array_key_exists('resizable', $options)) $this->setResizable((bool)$options['resizable']);
    }
    public function __destruct() { $this->document->unregisterWindow($this); $this->dispose(); }
    public function requestRedraw(): void { if (!$this->closed) Native::ffi()->sweet_window_redraw($this->native); }
    public function close(): void { if (!$this->closed) Native::ffi()->sweet_window_close($this->native); }
    private function dispose(): void { if (!$this->closed) { Native::ffi()->sweet_window_free($this->native); $this->closed = true; } }
    public function minimize(): void { Native::ffi()->sweet_window_minimize($this->native); }
    public function toggleMaximize(): void { Native::ffi()->sweet_window_toggle_maximize($this->native); }
    public function setResizable(bool $value): void { $this->resizable = $value; Native::ffi()->sweet_window_set_resizable($this->native, $value); }
    public function setResizeAxes(string $value): void { [$horizontal, $vertical] = self::parseResizeAxes($value); $this->resizeAxes = $value; Native::ffi()->sweet_window_set_resize_axes($this->native, $horizontal, $vertical); }
    public function setSafeArea(string $value): void { self::parseSafeArea($value); $this->safeArea = $value; }
    private static function parseResizeAxes(string $value): array { return match ($value) { 'both' => [true, true], 'horizontal' => [true, false], 'vertical' => [false, true], 'none' => [false, false], default => throw new \InvalidArgumentException("invalid resizeAxes '$value'; expected none, horizontal, vertical, or both") }; }
    private static function parseSafeArea(string $value): void { if (!in_array($value, ['none', 'top', 'bottom', 'both'], true)) throw new \InvalidArgumentException("invalid safeArea '$value'; expected none, top, bottom, or both"); }
    public function __get(string $name): mixed { return match ($name) { 'resizable' => $this->resizable, 'resizeAxes' => $this->resizeAxes, 'safeArea' => $this->safeArea, default => throw new \LogicException("Unknown window property: $name") }; }
    public function __set(string $name, mixed $value): void { if ($name === 'resizable') { $this->setResizable((bool)$value); return; } if ($name === 'resizeAxes') { $this->setResizeAxes((string)$value); return; } if ($name === 'safeArea') { $this->setSafeArea((string)$value); return; } throw new \LogicException("Unknown window property: $name"); }
    public function isClosed(): bool { return $this->closed || Native::ffi()->sweet_window_is_closed($this->native); }
    public function waitUntilClosed(): void { if (!$this->closed) Native::ffi()->sweet_window_wait_closed($this->native); }
    public function on(string $event, callable $callback): self { $this->listeners[$event][] = $callback; return $this; }
    public function pollEvents(): int
    {
        if ($this->closed) return 0;
        $ffi = Native::ffi(); $count = 0;
        while (true) {
            $event = $ffi->new('SweetEvent');
            if (!$ffi->sweet_window_poll_event($this->native, \FFI::addr($event))) break;
            $detail = new SweetEvent((int)$event->node_id, (float)$event->x, (float)$event->y, (float)$event->width, (float)$event->height);
            $name = match ((int)$event->kind) { 1 => 'click', 2 => 'resize', 3 => 'close', 4 => 'mousemove', 5 => 'mousedown', 6 => 'mouseup', default => 'unknown' };
            foreach ($this->listeners[$name] ?? [] as $callback) $callback($detail);
            if ($name === 'close') { $this->closed = true; continue; }
            if (in_array($name, ['click', 'mousemove', 'mousedown', 'mouseup'], true)) $this->document->dispatch($detail->nodeId, $name, $detail);
            $count++;
        }
        return $count;
    }
    public function run(): void { while (!$this->isClosed()) { $this->pollEvents(); usleep(8000); } $this->pollEvents(); }
}

final class SweetEvent
{
    public function __construct(public readonly int $nodeId, public readonly float $x, public readonly float $y, public readonly float $width, public readonly float $height) {}
}

final class Dominant
{
    private array $bindings = [];
    private array $eventListeners = ['beforeUpdate' => [], 'update' => []];
    public function __construct(private Document $document) {}
    public function binding(callable $get, ?callable $set = null): array { return ['__sweet_binding' => true, 'get' => $get, 'set' => $set]; }
    public function el(string|Node $type, array $props = [], mixed ...$children): Node
    {
        if (isset($props['children'])) { if ($children) throw new \InvalidArgumentException('Ambiguous children parameters'); $children = (array)$props['children']; unset($props['children']); }
        $element = $type instanceof Node ? $type : $this->document->createElement($type);
        foreach ($props as $key => $value) {
            if (str_starts_with((string)$key, 'on') && is_callable($value)) {
                $eventName = strtolower((string)$key);
                $eventName = preg_replace('/^on:?/', '', $eventName);
                if ($eventName === '') throw new \InvalidArgumentException("Invalid event prop: $key");
                $element->on($eventName, $value); continue;
            }
            if (($key === 'class' || $key === 'className') && is_array($value)) {
                $dynamic = array_filter($value, 'is_callable');
                $static = array_filter($value, fn($item) => !is_callable($item));
                $element->setClassName(implode(' ', $this->classValues($static)));
                if ($dynamic) $this->bind($element, 'class', fn() => array_merge($this->classValues($static), ...array_map(fn($get) => $this->classValues([$get()]), $dynamic)));
                continue;
            }
            if (is_callable($value)) { $this->bind($element, (string)$key, $value); continue; }
            if (is_array($value) && isset($value['__sweet_binding'])) { $this->bind($element, (string)$key, $value['get']); continue; }
            $this->apply($element, (string)$key, $value);
        }
        foreach ($this->flatten($children) as $child) { $node = $this->appendable($child); if ($node) $element->appendChild($node); }
        return $element;
    }
    public function text(callable $get): Node
    {
        $node = $this->document->createText('');
        $this->bindings[] = function() use ($node, $get) { $value = $get(); $node->setText(is_bool($value) || $value === null ? '' : (string)$value); };
        return $node;
    }
    public function comment(string $value = ''): Node { return $this->document->createComment($value); }
    public function child(callable $get): mixed
    {
        try { $value = $get(); if ($value === null || $value instanceof Node || is_array($value)) return $value; } catch (\Throwable) {}
        return $this->text($get);
    }
    public function portal(callable $get): Node
    {
        $anchor = $this->document->createComment('portal anchor'); $last = new \stdClass(); $lastNode = null;
        $this->bindings[] = function() use ($get, $anchor, &$last, &$lastNode) {
            $parent = $anchor->parent(); if (!$parent) return;
            $next = $get(); if ($next === $last) return;
            if ($lastNode instanceof Node) $lastNode->remove();
            $last = $next; $lastNode = $this->appendable($next);
            $siblings = $parent->children(); $idx = array_search($anchor->id, array_map(fn($node) => $node->id, $siblings), true);
            if ($lastNode) $parent->insertBefore($lastNode, $idx !== false ? ($siblings[$idx + 1] ?? null) : null);
        };
        return $anchor;
    }
    public function if(callable $condition, mixed $then, mixed $else = null): Node
    {
        $anchor = $this->document->createComment('if anchor'); $last = null; $shown = [];
        $this->bindings[] = function() use ($condition, $then, $else, $anchor, &$last, &$shown) {
            $parent = $anchor->parent(); if (!$parent) return;
            $next = (bool)$condition(); if ($last === $next) return;
            foreach ($shown as $node) $node->remove(); $shown = [];
            $siblings = $parent->children(); $idx = array_search($anchor->id, array_map(fn($n) => $n->id, $siblings), true);
            $before = $idx !== false ? ($siblings[$idx + 1] ?? null) : null;
            $values = $this->flatten([$next ? $then : $else]);
            foreach ($values as $value) { $node = $this->appendable($value); if ($node) { $parent->insertBefore($node, $before); $shown[] = $node; } }
            $last = $next;
        };
        return $anchor;
    }
    public function map(callable $get, mixed $separatorOrRender, ?callable $render = null): Node
    {
        if ($render === null) { $render = $separatorOrRender; $separator = null; }
        else $separator = $separatorOrRender;
        $anchor = $this->document->createComment('map anchor'); $last = null; $shown = [];
        $this->bindings[] = function() use ($get, $separator, $render, $anchor, &$last, &$shown) {
            $parent = $anchor->parent(); if (!$parent) return;
            $items = array_values((array)($get() ?? [])); if ($last === $items) return;
            foreach ($shown as $node) $node->remove(); $shown = [];
            $siblings = $parent->children(); $idx = array_search($anchor->id, array_map(fn($n) => $n->id, $siblings), true); $before = $idx !== false ? ($siblings[$idx + 1] ?? null) : null;
            foreach ($items as $i => $item) { if ($i > 0) { $sep = $this->appendable($separator); if ($sep) { $parent->insertBefore($sep, $before); $shown[] = $sep; } } foreach ($this->flatten([$render($item, $i)]) as $value) { $node = $this->appendable($value); if ($node) { $parent->insertBefore($node, $before); $shown[] = $node; } } }
            $last = $items;
        };
        return $anchor;
    }
    public function updateSync(): void
    {
        foreach ($this->eventListeners['beforeUpdate'] as $callback) $callback();
        foreach ($this->bindings as $binding) $binding();
        foreach ($this->eventListeners['update'] as $callback) $callback();
    }
    public function update(): void { $this->updateSync(); }
    public function on(string $event, callable $callback): void { if (isset($this->eventListeners[$event])) $this->eventListeners[$event][] = $callback; }
    public function off(string $event, callable $callback): void { if (isset($this->eventListeners[$event])) $this->eventListeners[$event] = array_values(array_filter($this->eventListeners[$event], fn($item) => $item !== $callback)); }
    public function usePlaceholderTag(string $name, mixed $value): array { return ['placeholderTag' => $name, 'value' => $value]; }
    public function html(mixed ...$args): never { throw new \LogicException('sweet has no HTML; build the tree with d.el'); }
    public function effect(callable $condition, callable $effect): void
    {
        $last = new \stdClass();
        $this->bindings[] = function() use ($condition, $effect, &$last) { $value = $condition(); if ($last !== $value) { $effect($value); $last = $value; } };
    }
    public function resolve(mixed $value): mixed { return is_callable($value) ? $value() : $value; }
    private function bind(Node $node, string $key, callable $get): void
    {
        $last = new \stdClass();
        $this->bindings[] = function() use ($node, $key, $get, &$last) { $value = $get(); if ($last !== $value) { $this->apply($node, $key, $value); $last = $value; } };
    }
    private function apply(Node $node, string $key, mixed $value): void
    {
        if ($key === 'class' || $key === 'className') { $node->setClassName(implode(' ', $this->classValues([$value]))); return; }
        if ($key === 'textContent') { $node->setText((string)$value); return; }
        if ($key === 'style') throw new \LogicException('sweet has no CSS style properties; use Tailwind classes');
        $attr = str_contains($key, '-') || in_array($key, ['id','value','placeholder','type','disabled','src','alt','title'], true);
        if ($attr) {
            if ($value === null || $value === false) { $node->removeAttribute($key); return; }
            if ($key === 'src' && strtolower($node->tagName()) === 'img') { $node->setImageSource((string)$value); return; }
            $node->setAttribute($key, is_bool($value) ? ($value ? 'true' : 'false') : (string)$value);
        }
    }
    private function appendable(mixed $value): ?Node { if ($value instanceof Node) return $value; if (is_bool($value) || $value === null || $value === '') return null; return $this->document->createText((string)$value); }
    private function flatten(array $values): array { $out=[]; foreach($values as $v) { if (is_array($v)) array_push($out,...$this->flatten($v)); else $out[]=$v; } return $out; }
    private function classValues(array $values): array { $out=[]; foreach($values as $value) { if (is_array($value)) array_push($out,...$this->classValues($value)); elseif (is_scalar($value) && !is_bool($value)) foreach(preg_split('/\s+/', trim((string)$value), -1, PREG_SPLIT_NO_EMPTY) ?: [] as $class) $out[]=$class; } return array_values(array_unique($out)); }
}

function dominant(Document $document): Dominant { return new Dominant($document); }
