export type ProviderTabOption = {
  value: string;
  label: string;
};

export type ProviderTabsLayout = {
  visible: ProviderTabOption[];
  overflow: ProviderTabOption[];
};

function clampVisibleCount(
  ordered: ProviderTabOption[],
  tabWidths: Record<string, number>,
  availableWidth: number,
  moreWidth: number,
) {
  if (ordered.length === 0 || availableWidth <= 0) {
    return 0;
  }

  let used = 0;
  let visibleCount = 0;

  for (let index = 0; index < ordered.length; index += 1) {
    const option = ordered[index];
    const tabWidth = tabWidths[option.value] ?? 0;
    const remainingCount = ordered.length - (index + 1);
    const reserveMore = remainingCount > 0 ? moreWidth : 0;

    if (visibleCount > 0 && used + tabWidth + reserveMore > availableWidth) {
      break;
    }

    if (visibleCount === 0 && tabWidth + reserveMore > availableWidth) {
      visibleCount = 1;
      break;
    }

    used += tabWidth;
    visibleCount += 1;
  }

  return Math.max(visibleCount, 1);
}

export function buildProviderTabsLayout(
  ordered: ProviderTabOption[],
  selectedValue: string,
  tabWidths: Record<string, number>,
  availableWidth: number,
  moreWidth: number,
): ProviderTabsLayout {
  if (ordered.length === 0) {
    return { visible: [], overflow: [] };
  }

  const visibleCount = clampVisibleCount(
    ordered,
    tabWidths,
    availableWidth,
    moreWidth,
  );
  const initialVisible = ordered.slice(0, visibleCount);
  const initialOverflow = ordered.slice(visibleCount);

  if (
    initialOverflow.length === 0 ||
    initialVisible.some((option) => option.value === selectedValue)
  ) {
    return {
      visible: initialVisible,
      overflow: initialOverflow,
    };
  }

  const selectedOverflowIndex = initialOverflow.findIndex(
    (option) => option.value === selectedValue,
  );
  if (selectedOverflowIndex === -1) {
    return {
      visible: initialVisible,
      overflow: initialOverflow,
    };
  }

  const selectedOption = initialOverflow[selectedOverflowIndex];
  if (initialVisible.length === 0) {
    return {
      visible: [selectedOption],
      overflow: initialOverflow.filter(
        (option) => option.value !== selectedValue,
      ),
    };
  }

  const displaced = initialVisible[initialVisible.length - 1];
  const nextVisible = [...initialVisible.slice(0, -1), selectedOption];
  const nextOverflow = [
    displaced,
    ...initialOverflow.filter((option) => option.value !== selectedValue),
  ];

  return {
    visible: nextVisible,
    overflow: nextOverflow,
  };
}
