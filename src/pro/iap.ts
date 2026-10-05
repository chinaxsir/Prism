//! 移动端 IAP 桥接：tauri-plugin-iap（StoreKit2 / Google Play Billing）
//! 购买成功后将收据交给 Rust → 授权服务端验签，再落授权。
//!
//! 商品 ID 需与 App Store Connect / Google Play Console 以及服务端
//! PRISM_APPLE_*_PRODUCTS / PRISM_GOOGLE_*_PRODUCTS 配置一致。

import {
  PurchaseState,
  acknowledgePurchase,
  getProducts,
  purchase,
  restorePurchases,
  type Product,
  type Purchase as IapPurchase,
} from "@choochmeque/tauri-plugin-iap-api";

import { submitReceipt, type Entitlement } from "@/api/ipc";

/** 终身买断商品 ID */
export const PRODUCT_LIFETIME = "prism_pro_lifetime";
/** 月度订阅商品 ID */
export const PRODUCT_SUBSCRIPTION_MONTHLY = "prism_pro_monthly";

export interface StoreProductMeta {
  id: string;
  type: "inapp" | "subs";
  label: string;
}

export const STORE_PRODUCTS: StoreProductMeta[] = [
  { id: PRODUCT_LIFETIME, type: "inapp", label: "终身买断" },
  { id: PRODUCT_SUBSCRIPTION_MONTHLY, type: "subs", label: "月度订阅" },
];

/** 拉取商品与本地化价格（供购买弹窗展示） */
export async function fetchStoreProducts(): Promise<Product[]> {
  const [one, subs] = await Promise.all([
    getProducts([PRODUCT_LIFETIME], "inapp"),
    getProducts([PRODUCT_SUBSCRIPTION_MONTHLY], "subs"),
  ]);
  return [...one.products, ...subs.products];
}

/** 上报一笔已完成购买，服务端验签后签发授权 */
async function reportPurchase(
  p: IapPurchase,
  subscription: boolean,
): Promise<Entitlement> {
  if (p.jwsRepresentation) {
    // iOS：JWS 由服务端锚定 Apple 根证书逐级验签
    return submitReceipt("apple", {
      signedTransaction: p.jwsRepresentation,
    });
  }
  // Android：服务端用服务账号调 Google Play Developer API 校验
  return submitReceipt("google", {
    purchaseToken: p.purchaseToken,
    productId: p.productId,
    subscription,
  });
}

/** 发起购买并完成服务端校验；Android 在校验通过后才 ack（失败 3 天自动退款） */
export async function buyProduct(
  productId: string,
  productType: "inapp" | "subs",
): Promise<Entitlement> {
  const { products } = await getProducts([productId], productType);
  if (!products.length) throw new Error("无法从商店获取商品信息");

  // Android 订阅必须携带 base plan 的 offerToken
  const options =
    productType === "subs"
      ? {
          offerToken:
            products[0].subscriptionOfferDetails?.[0]?.offerToken,
        }
      : undefined;

  const p = await purchase(productId, productType, options);
  if (p.purchaseState !== PurchaseState.PURCHASED) {
    throw new Error("购买未完成");
  }

  const entitlement = await reportPurchase(p, productType === "subs");

  if (!p.jwsRepresentation && !p.isAcknowledged) {
    await acknowledgePurchase(p.purchaseToken);
  }
  return entitlement;
}

/**
 * 恢复购买（换机/重装）：分别恢复订阅与买断，逐笔上报服务端。
 * 返回上报的交易笔数。
 */
export async function restoreAll(): Promise<number> {
  const [subsResult, inappResult] = await Promise.all([
    restorePurchases("subs"),
    restorePurchases("inapp"),
  ]);

  const subs = subsResult.purchases.filter(
    (p) => p.purchaseState === PurchaseState.PURCHASED,
  );
  const inapp = inappResult.purchases.filter(
    (p) => p.purchaseState === PurchaseState.PURCHASED,
  );

  for (const p of subs) {
    await reportPurchase(p, true);
  }
  for (const p of inapp) {
    await reportPurchase(p, false);
  }
  return subs.length + inapp.length;
}
