"use client";

import { memo, useRef, type ComponentPropsWithoutRef, type FC } from "react";
import {
  ComposerPrimitive,
  unstable_defaultDirectiveFormatter,
  unstable_useTriggerPopoverScopeContext,
  type Unstable_DirectiveFormatter,
  type Unstable_TriggerItem,
} from "@assistant-ui/react";
import { ChevronLeftIcon, ChevronRightIcon, SparklesIcon } from "lucide-react";
import { cn } from "@/lib/utils";

type IconComponent = FC<{ className?: string }>;

type DirectiveBehaviorProps = {
  formatter?: Unstable_DirectiveFormatter;
  onInserted?: (item: Unstable_TriggerItem) => void;
};

type ActionBehaviorProps = {
  formatter?: Unstable_DirectiveFormatter;
  onExecute: (item: Unstable_TriggerItem) => void;
  removeOnExecute?: boolean;
};

type ComposerTriggerPopoverProps = Omit<
  ComponentPropsWithoutRef<typeof ComposerPrimitive.Unstable_TriggerPopover>,
  "children"
> & {
  iconMap?: Record<string, IconComponent>;
  fallbackIcon?: IconComponent;
  backLabel?: string;
  emptyCategoriesLabel?: string;
  emptyItemsLabel?: string;
  loadingLabel?: string;
} & (
    | { directive: DirectiveBehaviorProps; action?: never }
    | { action: ActionBehaviorProps; directive?: never }
  );

function resolveIcon(
  iconKey: string | undefined,
  iconMap: Record<string, IconComponent> | undefined,
  fallback: IconComponent,
): IconComponent {
  return (iconKey && iconMap?.[iconKey]) || fallback;
}

const Categories: FC<{
  iconMap?: Record<string, IconComponent>;
  fallbackIcon: IconComponent;
  emptyLabel: string;
}> = ({ iconMap, fallbackIcon, emptyLabel }) => (
  <ComposerPrimitive.Unstable_TriggerPopoverCategories>
    {(categories) => (
      <div className="flex flex-col py-1">
        {categories.map((category) => {
          const Icon = resolveIcon(category.id, iconMap, fallbackIcon);
          return (
            <ComposerPrimitive.Unstable_TriggerPopoverCategoryItem
              key={category.id}
              categoryId={category.id}
              className="hover:bg-accent focus:bg-accent data-[highlighted]:bg-accent flex cursor-pointer items-center justify-between gap-2 px-3 py-2 text-sm outline-none transition-colors"
            >
              <span className="flex items-center gap-2">
                <Icon className="text-muted-foreground size-4" />
                {category.label}
              </span>
              <ChevronRightIcon className="text-muted-foreground size-4" />
            </ComposerPrimitive.Unstable_TriggerPopoverCategoryItem>
          );
        })}
        {categories.length === 0 && (
          <div className="text-muted-foreground px-3 py-2 text-sm">
            {emptyLabel}
          </div>
        )}
      </div>
    )}
  </ComposerPrimitive.Unstable_TriggerPopoverCategories>
);

const Items: FC<{
  iconMap?: Record<string, IconComponent>;
  fallbackIcon: IconComponent;
  backLabel: string;
  emptyLabel: string;
  loadingLabel: string;
}> = ({ iconMap, fallbackIcon, backLabel, emptyLabel, loadingLabel }) => {
  const { isLoading } = unstable_useTriggerPopoverScopeContext();

  return (
    <ComposerPrimitive.Unstable_TriggerPopoverItems>
      {(items) => (
        <div className="flex flex-col">
          <ComposerPrimitive.Unstable_TriggerPopoverBack className="text-muted-foreground hover:bg-accent flex cursor-pointer items-center gap-1.5 border-b px-3 py-2 text-xs uppercase tracking-wide outline-none transition-colors">
            <ChevronLeftIcon className="size-3.5" />
            {backLabel}
          </ComposerPrimitive.Unstable_TriggerPopoverBack>
          <div className="py-1">
            {items.map((item, index) => {
              const iconKey =
                typeof item.metadata?.icon === "string"
                  ? item.metadata.icon
                  : undefined;
              const Icon = resolveIcon(iconKey, iconMap, fallbackIcon);
              return (
                <ComposerPrimitive.Unstable_TriggerPopoverItem
                  key={item.id}
                  item={item}
                  index={index}
                  className="hover:bg-accent focus:bg-accent data-[highlighted]:bg-accent flex w-full cursor-pointer flex-col items-start gap-0.5 px-3 py-2 text-start outline-none transition-colors"
                >
                  <span className="flex items-center gap-2 text-sm font-medium">
                    <Icon className="text-primary size-3.5" />
                    {item.label}
                  </span>
                  {item.description && (
                    <span className="text-muted-foreground ms-5.5 text-xs leading-tight">
                      {item.description}
                    </span>
                  )}
                </ComposerPrimitive.Unstable_TriggerPopoverItem>
              );
            })}
            {items.length === 0 && (
              <div className="text-muted-foreground px-3 py-2 text-sm">
                {isLoading ? loadingLabel : emptyLabel}
              </div>
            )}
          </div>
        </div>
      )}
    </ComposerPrimitive.Unstable_TriggerPopoverItems>
  );
};

const ComposerTriggerPopoverImpl: FC<ComposerTriggerPopoverProps> = ({
  iconMap,
  fallbackIcon = SparklesIcon,
  backLabel = "Back",
  emptyCategoriesLabel = "No items available",
  emptyItemsLabel = "No matching items",
  loadingLabel = "Loading…",
  className,
  directive,
  action,
  ...props
}) => {
  const warnedRef = useRef(false);
  if (
    process.env.NODE_ENV !== "production" &&
    !warnedRef.current &&
    Boolean(directive) === Boolean(action)
  ) {
    warnedRef.current = true;
    console.warn(
      "ComposerTriggerPopover requires exactly one of directive or action.",
    );
  }

  return (
    <ComposerPrimitive.Unstable_TriggerPopover
      data-slot="composer-trigger-popover"
      className={cn(
        "aui-composer-trigger-popover bg-popover text-popover-foreground absolute start-0 bottom-full z-50 mb-2 w-72 overflow-hidden rounded-xl border shadow-lg",
        className,
      )}
      {...props}
    >
      {directive ? (
        <ComposerPrimitive.Unstable_TriggerPopover.Directive
          formatter={directive.formatter ?? unstable_defaultDirectiveFormatter}
          onInserted={directive.onInserted}
        />
      ) : action ? (
        <ComposerPrimitive.Unstable_TriggerPopover.Action
          formatter={action.formatter ?? unstable_defaultDirectiveFormatter}
          onExecute={action.onExecute}
          removeOnExecute={action.removeOnExecute}
        />
      ) : null}
      <Categories
        iconMap={iconMap}
        fallbackIcon={fallbackIcon}
        emptyLabel={emptyCategoriesLabel}
      />
      <Items
        iconMap={iconMap}
        fallbackIcon={fallbackIcon}
        backLabel={backLabel}
        emptyLabel={emptyItemsLabel}
        loadingLabel={loadingLabel}
      />
    </ComposerPrimitive.Unstable_TriggerPopover>
  );
};

export const ComposerTriggerPopover = memo(
  ComposerTriggerPopoverImpl,
) as FC<ComposerTriggerPopoverProps>;
