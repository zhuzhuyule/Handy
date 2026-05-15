import { Box, Flex, Heading, Text } from "@radix-ui/themes";
import React from "react";
import { Card, CardProps } from "./Card";

interface SettingsGroupProps {
  title?: React.ReactNode;
  titleClassName?: string;
  description?: React.ReactNode;
  actions?: React.ReactNode;
  children: React.ReactNode;
  cardProps?: CardProps;
  contentClassName?: string;
  framed?: boolean;
  noContent?: boolean;
}

export const SettingsGroup: React.FC<SettingsGroupProps> = ({
  title,
  titleClassName,
  description,
  children,
  actions,
  cardProps,
  contentClassName,
  framed = true,
  noContent,
}) => {
  const header = (
    <Box mb={noContent ? "0" : "2"}>
      <Flex justify="between" align="center">
        <Box>
          {title && (
            <Heading
              size="4"
              weight="bold"
              highContrast={!titleClassName}
              style={!titleClassName ? { color: "var(--gray-12)" } : undefined}
              className={titleClassName}
            >
              {title}
            </Heading>
          )}
          {description && (
            <Text size="2" color="gray" mt="1" style={{ display: "block" }}>
              {description}
            </Text>
          )}
        </Box>
        {actions && <Box>{actions}</Box>}
      </Flex>
    </Box>
  );

  const content = (
    <Flex direction="column" gap="1" className={contentClassName}>
      {React.Children.map(children, (child, index) => (
        <React.Fragment key={index}>{child}</React.Fragment>
      ))}
    </Flex>
  );

  if (framed) {
    return (
      <Card {...cardProps}>
        {header}
        {content}
      </Card>
    );
  }

  return (
    <Box>
      {header}
      {content}
    </Box>
  );
};
