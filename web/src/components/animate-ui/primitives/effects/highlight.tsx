
import * as React from 'react';
import { AnimatePresence, m as motion, type Transition } from 'motion/react';

import { cn } from '@/lib/utils';

type HighlightMode = 'children' | 'parent';

type Bounds = {
  top: number;
  left: number;
  width: number;
  height: number;
};

const DEFAULT_BOUNDS_OFFSET: Bounds = {
  top: 0,
  left: 0,
  width: 0,
  height: 0,
};

const DEFAULT_TRANSITION: Transition = {
  type: 'spring',
  stiffness: 350,
  damping: 35,
};

type HighlightContextType<T extends string> = {
  as?: keyof HTMLElementTagNameMap;
  mode: HighlightMode;
  activeValue: T | null;
  setActiveValue: (value: T | null) => void;
  setBounds: (bounds: DOMRect) => void;
  clearBounds: () => void;
  id: string;
  hover: boolean;
  click: boolean;
  className?: string;
  style?: React.CSSProperties;
  activeClassName?: string;
  setActiveClassName: (className: string) => void;
  transition?: Transition;
  disabled?: boolean;
  enabled?: boolean;
  exitDelay?: number;
  forceUpdateBounds?: boolean;
};

const HighlightContext = React.createContext<
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  HighlightContextType<any> | undefined
>(undefined);

function useHighlight<T extends string>(): HighlightContextType<T> {
  const context = React.useContext(HighlightContext);
  if (!context) {
    throw new Error('useHighlight must be used within a HighlightProvider');
  }
  return context as unknown as HighlightContextType<T>;
}

type BaseHighlightProps<T extends React.ElementType = 'div'> = {
  as?: T;
  ref?: React.Ref<HTMLDivElement>;
  mode?: HighlightMode;
  value?: string | null;
  defaultValue?: string | null;
  onValueChange?: (value: string | null) => void;
  className?: string;
  style?: React.CSSProperties;
  transition?: Transition;
  hover?: boolean;
  click?: boolean;
  disabled?: boolean;
  enabled?: boolean;
  exitDelay?: number;
};

type ParentModeHighlightProps = {
  boundsOffset?: Partial<Bounds>;
  containerClassName?: string;
  forceUpdateBounds?: boolean;
};

type ControlledParentModeHighlightProps<T extends React.ElementType = 'div'> =
  BaseHighlightProps<T> &
    ParentModeHighlightProps & {
      mode: 'parent';
      controlledItems: true;
      children: React.ReactNode;
    };

type ControlledChildrenModeHighlightProps<T extends React.ElementType = 'div'> =
  BaseHighlightProps<T> & {
    mode?: 'children' | undefined;
    controlledItems: true;
    children: React.ReactNode;
  };

type UncontrolledParentModeHighlightProps<T extends React.ElementType = 'div'> =
  BaseHighlightProps<T> &
    ParentModeHighlightProps & {
      mode: 'parent';
      controlledItems?: false;
      itemsClassName?: string;
      children: React.ReactElement | React.ReactElement[];
    };

type UncontrolledChildrenModeHighlightProps<
  T extends React.ElementType = 'div',
> = BaseHighlightProps<T> & {
  mode?: 'children';
  controlledItems?: false;
  itemsClassName?: string;
  children: React.ReactElement | React.ReactElement[];
};

type HighlightProps<T extends React.ElementType = 'div'> =
  | ControlledParentModeHighlightProps<T>
  | ControlledChildrenModeHighlightProps<T>
  | UncontrolledParentModeHighlightProps<T>
  | UncontrolledChildrenModeHighlightProps<T>;

function Highlight<T extends React.ElementType = 'div'>({
  ref,
  ...props
}: HighlightProps<T>) {
  const {
    as: Component = 'div',
    children,
    value,
    defaultValue,
    onValueChange,
    className,
    style,
    transition = DEFAULT_TRANSITION,
    hover = false,
    click = true,
    enabled = true,
    controlledItems,
    disabled = false,
    exitDelay = 200,
    mode = 'children',
  } = props;

  const localRef = React.useRef<HTMLDivElement>(null);
  React.useImperativeHandle(ref, () => localRef.current as HTMLDivElement);

  const propsBoundsOffset = (props as ParentModeHighlightProps)?.boundsOffset;
  const boundsOffset = propsBoundsOffset ?? DEFAULT_BOUNDS_OFFSET;
  const boundsOffsetTop = boundsOffset.top ?? 0;
  const boundsOffsetLeft = boundsOffset.left ?? 0;
  const boundsOffsetWidth = boundsOffset.width ?? 0;
  const boundsOffsetHeight = boundsOffset.height ?? 0;

  const boundsOffsetRef = React.useRef({
    top: boundsOffsetTop,
    left: boundsOffsetLeft,
    width: boundsOffsetWidth,
    height: boundsOffsetHeight,
  });

  React.useEffect(() => {
    boundsOffsetRef.current = {
      top: boundsOffsetTop,
      left: boundsOffsetLeft,
      width: boundsOffsetWidth,
      height: boundsOffsetHeight,
    };
  }, [
    boundsOffsetTop,
    boundsOffsetLeft,
    boundsOffsetWidth,
    boundsOffsetHeight,
  ]);

  const [activeValue, setActiveValue] = React.useState<string | null>(
    value ?? defaultValue ?? null,
  );
  // A new value or defaultValue prop takes over from whatever was picked locally.
  const [syncedProps, setSyncedProps] = React.useState({ value, defaultValue });
  if (
    syncedProps.value !== value ||
    syncedProps.defaultValue !== defaultValue
  ) {
    setSyncedProps({ value, defaultValue });
    if (value !== undefined) setActiveValue(value);
    else if (defaultValue !== undefined) setActiveValue(defaultValue);
  }
  const [boundsState, setBoundsState] = React.useState<Bounds | null>(null);
  const [activeClassNameState, setActiveClassNameState] =
    React.useState<string>('');

  // Latest value, so several calls in one event compare against each other.
  const latestActiveValueRef = React.useRef(activeValue);
  React.useLayoutEffect(() => {
    latestActiveValueRef.current = activeValue;
  }, [activeValue]);

  const safeSetActiveValue = React.useCallback(
    (id: string | null) => {
      if (latestActiveValueRef.current === id) return;
      latestActiveValueRef.current = id;
      setActiveValue(id);
      onValueChange?.(id);
    },
    [onValueChange],
  );

  const safeSetBoundsRef = React.useRef<
    ((bounds: DOMRect) => void) | undefined
  >(undefined);

  React.useEffect(() => {
    safeSetBoundsRef.current = (bounds: DOMRect) => {
      if (!localRef.current) return;

      const containerRect = localRef.current.getBoundingClientRect();
      const offset = boundsOffsetRef.current;
      const newBounds: Bounds = {
        top: bounds.top - containerRect.top + offset.top,
        left: bounds.left - containerRect.left + offset.left,
        width: bounds.width + offset.width,
        height: bounds.height + offset.height,
      };

      setBoundsState((prev) => {
        if (
          prev &&
          prev.top === newBounds.top &&
          prev.left === newBounds.left &&
          prev.width === newBounds.width &&
          prev.height === newBounds.height
        ) {
          return prev;
        }
        return newBounds;
      });
    };
  });

  const safeSetBounds = React.useCallback((bounds: DOMRect) => {
    safeSetBoundsRef.current?.(bounds);
  }, []);

  const clearBounds = React.useCallback(() => {
    setBoundsState((prev) => (prev === null ? prev : null));
  }, []);

  const id = React.useId();

  React.useEffect(() => {
    if (mode !== 'parent') return;
    const container = localRef.current;
    if (!container) return;

    const onScroll = () => {
      if (!activeValue) return;
      const activeEl = container.querySelector<HTMLElement>(
        `[data-value="${activeValue}"][data-highlight="true"]`,
      );
      if (activeEl)
        safeSetBoundsRef.current?.(activeEl.getBoundingClientRect());
    };

    container.addEventListener('scroll', onScroll, { passive: true });
    return () => container.removeEventListener('scroll', onScroll);
  }, [mode, activeValue]);

  const render = (children: React.ReactNode) => {
    if (mode === 'parent') {
      return (
        <Component
          ref={localRef}
          data-slot="motion-highlight-container"
          style={{ position: 'relative', zIndex: 1 }}
          className={(props as ParentModeHighlightProps)?.containerClassName}
        >
          <AnimatePresence initial={false} mode="wait">
            {boundsState && (
              <motion.div
                data-slot="motion-highlight"
                layout
                animate={{ opacity: 1 }}
                initial={{ opacity: 0 }}
                exit={{
                  opacity: 0,
                  transition: {
                    ...transition,
                    delay: (transition?.delay ?? 0) + (exitDelay ?? 0) / 1000,
                  },
                }}
                transition={transition}
                style={{
                  position: 'absolute',
                  zIndex: 0,
                  ...style,
                  // The box moves and resizes with a layout animation, which motion runs as transforms.
                  top: boundsState.top,
                  left: boundsState.left,
                  width: boundsState.width,
                  height: boundsState.height,
                }}
                className={cn(className, activeClassNameState)}
              />
            )}
          </AnimatePresence>
          {children}
        </Component>
      );
    }

    return children;
  };

  const forceUpdateBounds = (props as ParentModeHighlightProps)
    ?.forceUpdateBounds;
  const contextValue = React.useMemo(
    () => ({
      mode,
      activeValue,
      setActiveValue: safeSetActiveValue,
      id,
      hover,
      click,
      className,
      style,
      transition,
      disabled,
      enabled,
      exitDelay,
      setBounds: safeSetBounds,
      clearBounds,
      activeClassName: activeClassNameState,
      setActiveClassName: setActiveClassNameState,
      forceUpdateBounds,
    }),
    [
      mode,
      activeValue,
      safeSetActiveValue,
      id,
      hover,
      click,
      className,
      style,
      transition,
      disabled,
      enabled,
      exitDelay,
      safeSetBounds,
      clearBounds,
      activeClassNameState,
      forceUpdateBounds,
    ],
  );

  return (
    <HighlightContext.Provider value={contextValue}>
      {enabled
        ? controlledItems
          ? render(children)
          : render(
              // Children.map keys each wrapper from its child's own key.
              React.Children.map(children, (child) => (
                <HighlightItem className={props?.itemsClassName}>
                  {child}
                </HighlightItem>
              )),
            )
        : children}
    </HighlightContext.Provider>
  );
}

function getNonOverridingDataAttributes(
  element: React.ReactElement,
  dataAttributes: Record<string, unknown>,
): Record<string, unknown> {
  return Object.keys(dataAttributes).reduce<Record<string, unknown>>(
    (acc, key) => {
      if ((element.props as Record<string, unknown>)[key] === undefined) {
        acc[key] = dataAttributes[key];
      }
      return acc;
    },
    {},
  );
}

type ExtendedChildProps = React.ComponentProps<'div'> & {
  id?: string;
  ref?: React.Ref<HTMLElement>;
  'data-active'?: string;
  'data-value'?: string;
  'data-disabled'?: boolean;
  'data-highlight'?: boolean;
  'data-slot'?: string;
};

type HighlightItemProps<T extends React.ElementType = 'div'> =
  React.ComponentProps<T> & {
    as?: T;
    children: React.ReactElement;
    id?: string;
    value?: string;
    className?: string;
    style?: React.CSSProperties;
    transition?: Transition;
    activeClassName?: string;
    disabled?: boolean;
    exitDelay?: number;
    asChild?: boolean;
    forceUpdateBounds?: boolean;
  };

// Mouse handlers that move the highlight onto (true) or off (false) an item, keeping the child's own handlers.
function itemHandlers(
  trigger: 'hover' | 'click' | null,
  element: React.ReactElement<ExtendedChildProps>,
  activate: (active: boolean) => void,
) {
  if (trigger === 'hover') {
    return {
      onMouseEnter: (e: React.MouseEvent<HTMLDivElement>) => {
        activate(true);
        element.props.onMouseEnter?.(e);
      },
      onMouseLeave: (e: React.MouseEvent<HTMLDivElement>) => {
        activate(false);
        element.props.onMouseLeave?.(e);
      },
    };
  }
  if (trigger === 'click') {
    return {
      onClick: (e: React.MouseEvent<HTMLDivElement>) => {
        activate(true);
        element.props.onClick?.(e);
      },
    };
  }
  return {};
}

type HighlightBackgroundProps = {
  show: boolean;
  style?: React.CSSProperties;
  activeClassName?: string;
  transition?: Transition;
  exitDelay?: number;
  dataAttributes: Record<string, unknown>;
};

// The sliding background behind the active item in children mode.
function HighlightBackground({
  show,
  style,
  activeClassName,
  transition,
  exitDelay,
  dataAttributes,
}: HighlightBackgroundProps) {
  const {
    id: contextId,
    className: contextClassName,
    style: contextStyle,
    transition: contextTransition,
    exitDelay: contextExitDelay,
  } = useHighlight();
  const itemTransition = transition ?? contextTransition;
  const delay = (exitDelay ?? contextExitDelay ?? 0) / 1000;

  return (
    <AnimatePresence initial={false} mode="wait">
      {show && (
        <motion.div
          layoutId={`transition-background-${contextId}`}
          data-slot="motion-highlight"
          style={{
            position: 'absolute',
            zIndex: 0,
            ...contextStyle,
            ...style,
          }}
          className={cn(contextClassName, activeClassName)}
          transition={itemTransition}
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{
            opacity: 0,
            transition: {
              ...itemTransition,
              delay: (itemTransition?.delay ?? 0) + delay,
            },
          }}
          {...dataAttributes}
        />
      )}
    </AnimatePresence>
  );
}

// In parent mode, keeps the shared highlight sized to the active item (every frame when forced).
function useItemBounds(
  localRef: React.RefObject<HTMLDivElement | null>,
  {
    isActive,
    activeClassName,
    forceUpdateBounds,
  }: {
    isActive: boolean;
    activeClassName?: string;
    forceUpdateBounds?: boolean;
  },
) {
  const {
    activeValue,
    mode,
    setBounds,
    clearBounds,
    forceUpdateBounds: contextForceUpdateBounds,
    setActiveClassName,
  } = useHighlight();

  React.useEffect(() => {
    if (mode !== 'parent') return;
    let rafId: number;
    let previousBounds: Bounds | null = null;
    const shouldUpdateBounds =
      forceUpdateBounds === true ||
      (contextForceUpdateBounds && forceUpdateBounds !== false);

    const updateBounds = () => {
      if (!localRef.current) return;

      const bounds = localRef.current.getBoundingClientRect();

      if (shouldUpdateBounds) {
        if (
          previousBounds &&
          previousBounds.top === bounds.top &&
          previousBounds.left === bounds.left &&
          previousBounds.width === bounds.width &&
          previousBounds.height === bounds.height
        ) {
          rafId = requestAnimationFrame(updateBounds);
          return;
        }
        previousBounds = bounds;
        rafId = requestAnimationFrame(updateBounds);
      }

      setBounds(bounds);
    };

    if (isActive) {
      updateBounds();
      setActiveClassName(activeClassName ?? '');
    } else if (!activeValue) clearBounds();

    if (shouldUpdateBounds) return () => cancelAnimationFrame(rafId);
  }, [
    mode,
    isActive,
    activeValue,
    setBounds,
    clearBounds,
    activeClassName,
    setActiveClassName,
    forceUpdateBounds,
    contextForceUpdateBounds,
    localRef,
  ]);
}

// The item's highlight value: explicit id/value first, then the child's own data-value or id.
function resolveItemValue(
  element: React.ReactElement<ExtendedChildProps>,
  explicit: string | undefined,
  fallback: string,
): string {
  return (
    explicit ?? element.props?.['data-value'] ?? element.props?.id ?? fallback
  );
}

function HighlightItem<T extends React.ElementType>({
  ref,
  as,
  children,
  id,
  value,
  className,
  style,
  transition,
  disabled = false,
  activeClassName,
  exitDelay,
  asChild = false,
  forceUpdateBounds,
  ...props
}: HighlightItemProps<T>) {
  const itemId = React.useId();
  const { activeValue, setActiveValue, mode, hover, click, enabled } =
    useHighlight();

  const Component = as ?? 'div';
  const element = children as React.ReactElement<ExtendedChildProps>;
  const childValue = resolveItemValue(element, id ?? value, itemId);
  const isActive = activeValue === childValue;

  const localRef = React.useRef<HTMLDivElement>(null);
  React.useImperativeHandle(ref, () => localRef.current as HTMLDivElement);

  const refCallback = React.useCallback((node: HTMLElement | null) => {
    localRef.current = node as HTMLDivElement;
  }, []);

  useItemBounds(localRef, {
    isActive,
    activeClassName,
    forceUpdateBounds,
  });

  if (!React.isValidElement(children)) return children;

  const dataAttributes = {
    'data-active': String(isActive),
    'aria-selected': isActive,
    'data-disabled': disabled,
    'data-value': childValue,
    'data-highlight': true,
  };

  const commonHandlers = itemHandlers(
    hover ? 'hover' : click ? 'click' : null,
    element,
    (next) => setActiveValue(next ? childValue : null),
  );

  const background = (
    <HighlightBackground
      show={isActive && !disabled}
      style={style}
      activeClassName={activeClassName}
      transition={transition}
      exitDelay={exitDelay}
      dataAttributes={dataAttributes}
    />
  );

  const childAttributes = (slot: string) =>
    getNonOverridingDataAttributes(element, {
      ...dataAttributes,
      'data-slot': slot,
    });

  if (asChild && mode === 'children') {
    return React.cloneElement(
      element,
      {
        key: childValue,
        ref: refCallback,
        className: cn('relative', element.props.className),
        ...childAttributes('motion-highlight-item-container'),
        ...commonHandlers,
        ...props,
      },
      <>
        {background}

        <Component
          data-slot="motion-highlight-item"
          style={{ position: 'relative', zIndex: 1 }}
          className={className}
          {...dataAttributes}
        >
          {children}
        </Component>
      </>,
    );
  }

  if (asChild) {
    return React.cloneElement(element, {
      ref: refCallback,
      ...childAttributes('motion-highlight-item'),
      ...commonHandlers,
    });
  }

  if (!enabled) return children;

  return (
    <Component
      key={childValue}
      ref={localRef}
      data-slot="motion-highlight-item-container"
      className={cn(mode === 'children' && 'relative', className)}
      {...dataAttributes}
      {...props}
      {...commonHandlers}
    >
      {mode === 'children' && background}

      {React.cloneElement(element, {
        style: { position: 'relative', zIndex: 1 },
        className: element.props.className,
        ...childAttributes('motion-highlight-item'),
      })}
    </Component>
  );
}

export {
  Highlight,
  HighlightItem,
  useHighlight,
  type HighlightProps,
  type HighlightItemProps,
};
