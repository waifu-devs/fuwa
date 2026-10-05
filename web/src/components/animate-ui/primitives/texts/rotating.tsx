
import * as React from 'react';
import { m as motion, AnimatePresence, type HTMLMotionProps } from 'motion/react';

import {
  useIsInView,
  type UseIsInViewOptions,
} from '@/hooks/use-is-in-view';
import { getStrictContext } from '@/lib/get-strict-context';

type RotatingTextContextType = {
  currentText: string;
  y: number;
  isInView: boolean;
};

const [RotatingTextProvider, useRotatingText] =
  getStrictContext<RotatingTextContextType>('RotatingTextContext');

type RotatingTextContainerProps = React.ComponentProps<'div'> & {
  text: string | string[];
  duration?: number;
  y?: number;
  delay?: number;
} & UseIsInViewOptions;

function RotatingTextContainer({
  ref,
  text,
  y = -50,
  duration = 2000,
  delay = 0,
  style,
  inView = false,
  inViewMargin = '0px',
  inViewOnce = true,
  ...props
}: RotatingTextContainerProps) {
  const [index, setIndex] = React.useState(0);

  const { ref: localRef, isInView } = useIsInView(
    ref as React.Ref<HTMLDivElement>,
    {
      inView,
      inViewOnce,
      inViewMargin,
    },
  );

  React.useEffect(() => {
    if (!Array.isArray(text)) return;
    if (inView && !isInView) return;

    // One timer at a time: the first after `delay`, then one every `duration`.
    let timeoutId = setTimeout(function tick() {
      setIndex((prev) => (prev + 1) % text.length);
      timeoutId = setTimeout(tick, duration);
    }, delay);

    return () => clearTimeout(timeoutId);
  }, [text, duration, delay, inView, isInView]);

  const currentText = Array.isArray(text) ? text[index] : text;

  return (
    <RotatingTextProvider value={{ currentText, y, isInView }}>
      <div
        ref={localRef}
        style={{
          overflow: 'hidden',
          paddingBlock: '0.25rem',
          ...style,
        }}
        {...props}
      />
    </RotatingTextProvider>
  );
}

type RotatingTextProps = Omit<HTMLMotionProps<'div'>, 'children'>;

function RotatingText({
  transition = { duration: 0.3, ease: 'easeOut' },
  ...props
}: RotatingTextProps) {
  const { currentText, y, isInView } = useRotatingText();

  return (
    <AnimatePresence mode="wait">
      {isInView && (
        <motion.div
          key={currentText}
          transition={transition}
          initial={{ opacity: 0, y: -y }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0, y }}
          {...props}
        >
          {currentText}
        </motion.div>
      )}
    </AnimatePresence>
  );
}

export {
  RotatingTextContainer,
  RotatingText,
  useRotatingText,
  type RotatingTextContainerProps,
  type RotatingTextProps,
  type RotatingTextContextType,
};
