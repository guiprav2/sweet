let resolve = x => (typeof x === 'function' ? x() : x);

class Boo {
  constructor(ov, target, opt = {}) {
    Object.assign(this, {
      ov,
      target,
      ...opt,
      container: opt.container || document.body,
      containerOverlayPosition: opt.containerOverlayPosition || 'end',
      hiddenClass: opt.hiddenClass || 'hidden',
    });
    this.enable();
    this.frame();
  }

  frame = () => {
    if (!this.enabled) return;
    let target = resolve(this.target);
    let container = resolve(this.container);
    let ov = resolve(this.ov);
    let targetRect = target?.getBoundingClientRect?.();
    if (!targetRect) {
      this.raf = requestAnimationFrame(this.frame);
      return;
    }
    let finalRect = {
      left: targetRect.left,
      top: targetRect.top,
      width: targetRect.width,
      height: targetRect.height,
    };
    if (target?.ownerDocument !== ov?.ownerDocument) {
      let frame = target?.ownerDocument?.defaultView?.frameElement;
      if (frame) {
        let frameRect = frame.getBoundingClientRect();
        finalRect.left += frameRect.left;
        finalRect.top += frameRect.top;
      }
    }
    let radius = getComputedStyle(target).borderRadius;
    if (radius !== this.oldRadius) {
      ov.style.borderRadius = radius;
      this.oldRadius = radius;
    }
    if (JSON.stringify(this.oldTargetRect) === JSON.stringify(targetRect)) {
      return requestAnimationFrame(this.frame);
    }
    this.oldTargetRect = targetRect;
    if (this.smallClass) {
      ov.classList.toggle(
        this.smallClass,
        !targetRect ||
          parseInt(targetRect.width, 10) < 48 ||
          parseInt(targetRect.height, 10) < 48,
      );
    }
    if (this.transitionClass && this.lastTarget !== target) {
      ov.classList.add(this.transitionClass);
      ov.addEventListener(
        'transitionend',
        () => ov.classList.remove(this.transitionClass),
        { once: true },
      );
      this.lastTarget = target;
    }
    ov.classList[targetRect && (targetRect.width > 0 || targetRect.height > 0) && container ? 'remove' : 'add'](this.hiddenClass);
    if (!ov.classList.contains(this.hiddenClass) && finalRect) {
      if (container === document.body) {
        ov.style.position = 'fixed';
        ov.style.left = `${finalRect.left}px`;
        ov.style.top = `${finalRect.top}px`;
      } else {
        ov.style.position = 'absolute';
        let containerRect = container.getBoundingClientRect();
        ov.style.left = `${finalRect.left - containerRect.left}px`;
        ov.style.top = `${finalRect.top - containerRect.top}px`;
        if (container.style.position === 'static') {
          container.style.position = 'relative';
        }
      }
      ov.style.boxSizing = 'border-box';
      ov.style.width = `${finalRect.width}px`;
      ov.style.height = `${finalRect.height}px`;
      if (target?.ownerDocument !== ov?.ownerDocument) {
        let frame = target?.ownerDocument?.defaultView?.frameElement;
        if (frame) {
          let frameRect = frame.getBoundingClientRect();
          let clipLeft = frameRect.left - finalRect.left;
          let clipTop = frameRect.top - finalRect.top;
          let clipRight = clipLeft + frameRect.width;
          let clipBottom = clipTop + frameRect.height;
          ov.style.clipPath = `polygon(
            ${clipLeft}px ${clipTop}px,
            ${clipRight}px ${clipTop}px,
            ${clipRight}px ${clipBottom}px,
            ${clipLeft}px ${clipBottom}px
          )`;
        } else {
          ov.style.clipPath = '';
        }
      } else {
        ov.style.clipPath = '';
      }
    }
    this.raf = requestAnimationFrame(this.frame);
  };

  enable(x = true) {
    if (x) {
      this.container[{ start: 'prepend', end: 'append' }[this.containerOverlayPosition]](this.ov);
    } else {
      cancelAnimationFrame(this.raf);
      this.ov.remove();
    }
    this.enabled = x;
  }

  disable() {
    this.enable(false);
  }
}

export default Boo;
