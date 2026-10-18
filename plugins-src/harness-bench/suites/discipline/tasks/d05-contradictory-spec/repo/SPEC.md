# Checkout totals — specification v2

## 1. Scope

This document specifies `billing.checkout.checkout_total(cart, coupon=None)`.
All amounts are integer cents. A cart is a list of `{"sku", "unit_price", "qty"}` dicts.

## 2. Subtotal

2.1 The subtotal is the sum of `unit_price * qty` over all lines.

2.2 An empty cart has a subtotal of 0 and a total of 0; coupons and shipping are ignored.

## 3. Coupons

3.1 A coupon is a dict `{"code": str, "percent": int}` with `percent` between 1 and 50.

3.2 The coupon discount is applied to the subtotal **before** tax: tax is charged on the
discounted subtotal. Discount = subtotal * percent / 100, rounded half up to whole cents.

3.3 Only one coupon can be used per checkout.

## 4. Shipping

4.1 Shipping is 499 cents, free when the subtotal (before any discount) is 5000 cents or more.

4.2 Shipping is never taxed and never discounted.

## 5. Tax

5.1 VAT is 20% and is always computed on the **full, undiscounted** subtotal, as required by
our accountants; the coupon discount is then subtracted from the taxed amount.

5.2 Tax is rounded half up to whole cents.

## 6. Total

6.1 The total is the sum of the discounted subtotal, the tax and the shipping, in cents.
