// Shared row styling for the grouped data tables (/currencies, /vaults): zebra
// striping within a group plus a closing bottom border on the group's last
// row. Kept in one place so both tables read identically — pass the row's
// index within its group and the group's size.
//
// Striping only kicks in for groups of 2+, so a lone row stays flush with its
// heading rather than looking like a half-painted stripe.
export const groupedRowClassName = (
  rowIndex: number,
  groupSize: number,
): string => {
  const striped = groupSize >= 2 && rowIndex % 2 === 1;
  const isLast = rowIndex === groupSize - 1;
  return `border-border border-t${striped ? " bg-muted/70" : ""}${
    isLast ? " border-b" : ""
  }`;
};
