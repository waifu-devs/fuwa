
import * as React from 'react';

type DataStateValue = string | boolean | null;

function parseDatasetValue(value: string | null): DataStateValue {
  if (value === null) return null;
  if (value === '' || value === 'true') return true;
  if (value === 'false') return false;
  return value;
}

function useDataState<T extends HTMLElement = HTMLElement>(
  key: string,
  forwardedRef?: React.Ref<T | null>,
  onChange?: (value: DataStateValue) => void,
): [DataStateValue, React.RefObject<T | null>] {
  const localRef = React.useRef<T | null>(null);
  React.useImperativeHandle(forwardedRef, () => localRef.current as T);

  const getSnapshot = (): DataStateValue => {
    const el = localRef.current;
    return el ? parseDatasetValue(el.getAttribute(`data-${key}`)) : null;
  };

  // Told straight from the attribute changing, not after a render.
  const onChangeRef = React.useRef(onChange);
  React.useLayoutEffect(() => {
    onChangeRef.current = onChange;
  });

  const subscribe = (callback: () => void) => {
    const el = localRef.current;
    if (!el) return () => {};
    const observer = new MutationObserver((records) => {
      for (const record of records) {
        if (record.attributeName === `data-${key}`) {
          onChangeRef.current?.(parseDatasetValue(el.getAttribute(`data-${key}`)));
          callback();
          break;
        }
      }
    });
    observer.observe(el, {
      attributes: true,
      attributeFilter: [`data-${key}`],
    });
    return () => observer.disconnect();
  };

  const value = React.useSyncExternalStore(subscribe, getSnapshot);

  return [value, localRef];
}

export { useDataState, type DataStateValue };
